//!
//!
//! This file contains the main structures for a NVMe controller
use nix::errno::Errno;
use once_cell::sync::OnceCell;
use std::{convert::From, os::raw::c_void, ptr::NonNull, sync::Arc};

use spdk_sys::{
    spdk_for_each_channel,
    spdk_for_each_channel_continue,
    spdk_io_channel_iter,
    spdk_io_channel_iter_get_channel,
    spdk_io_channel_iter_get_ctx,
    spdk_io_channel_iter_get_io_device,
    spdk_io_device_register,
    spdk_io_device_unregister,
    spdk_nvme_ctrlr,
    spdk_nvme_ctrlr_get_ns,
    spdk_nvme_ctrlr_process_admin_completions,
    spdk_nvme_ctrlr_reset,
    spdk_nvme_detach,
};

use crate::{
    bdev::dev::nvmx::{
        channel::{NvmeControllerIoChannel, NvmeIoChannel},
        nvme_bdev_running_config,
        uri::NvmeControllerContext,
        NvmeNamespace,
        NVME_CONTROLLERS,
    },
    core::{mempool::MemoryPool, poller, CoreError, IoCompletionCallback},
};

const RESET_CTX_POOL_SIZE: u64 = 1024 - 1;

// Memory pool for keeping context during controller resets.
static RESET_CTX_POOL: OnceCell<MemoryPool<ResetCtx>> = OnceCell::new();

struct ResetCtx {
    name: String,
    cb: IoCompletionCallback,
    cb_arg: *const c_void,
    spdk_handle: *mut spdk_nvme_ctrlr,
}

impl<'a> NvmeControllerInner<'a> {
    fn new(ctrlr: NonNull<spdk_nvme_ctrlr>) -> Self {
        let ctx = ctrlr.as_ptr().cast();

        let adminq_poller = poller::Builder::new()
            .with_name("nvme_poll_adminq")
            .with_interval(
                nvme_bdev_running_config().nvme_adminq_poll_period_us,
            )
            .with_poll_fn(move || nvme_poll_adminq(ctx))
            .build();

        Self {
            ctrlr,
            adminq_poller,
            namespaces: Vec::new(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum NvmeControllerState {
    Initializing,
    Running,
    Resetting,
    Destroying,
}

impl ToString for NvmeControllerState {
    fn to_string(&self) -> String {
        match *self {
            NvmeControllerState::Initializing => "Initializing",
            NvmeControllerState::Running => "Running",
            NvmeControllerState::Resetting => "Resetting",
            NvmeControllerState::Destroying => "Destroying",
        }
        .parse()
        .unwrap()
    }
}

#[derive(Debug)]
pub struct NvmeControllerInner<'a> {
    namespaces: Vec<Arc<NvmeNamespace>>,
    ctrlr: NonNull<spdk_nvme_ctrlr>,
    adminq_poller: poller::Poller<'a>,
}
/*
 * NVME controller implementation.
 */
#[derive(Debug)]
pub struct NvmeController<'a> {
    name: String,
    id: u64,
    prchk_flags: u32,
    pub(crate) state: NvmeControllerState,
    inner: Option<NvmeControllerInner<'a>>,
}

unsafe impl<'a> Send for NvmeController<'a> {}
unsafe impl<'a> Sync for NvmeController<'a> {}

impl<'a> NvmeController<'a> {
    /// Creates a new NVMe controller with the given name.
    pub fn new(name: &str, prchk_flags: u32) -> Option<Self> {
        let l = NvmeController {
            name: String::from(name),
            id: 0,
            prchk_flags,
            state: NvmeControllerState::Initializing,
            inner: None,
        };

        debug!("{}: new NVMe controller created", l.get_name());
        Some(l)
    }

    /// returns the name of the current controller
    pub fn get_name(&self) -> String {
        self.name.clone()
    }

    /// returns the protection flags the controller is created with
    pub fn flags(&self) -> u32 {
        self.prchk_flags
    }

    /// returns the ID of the controller
    pub fn id(&self) -> u64 {
        assert_ne!(self.id, 0, "Controller ID is not yet initialized");
        self.id
    }

    fn set_id(&mut self, id: u64) -> u64 {
        assert_ne!(id, 0, "Controller ID can't be zero");
        self.id = id;
        debug!("{} ID set to 0x{:X}", self.name, self.id);
        id
    }

    fn set_state(&mut self, new_state: NvmeControllerState) {
        info!(
            "{} Transitioned from state {:?} to {:?}",
            self.name, self.state, new_state
        );
    }

    // As of now, only 1 namespace per controller is supported.
    pub fn namespace(&self) -> Option<Arc<NvmeNamespace>> {
        let inner = self
            .inner
            .as_ref()
            .expect("(BUG) no inner NVMe controller defined yet");

        if let Some(ns) = inner.namespaces.get(0) {
            Some(Arc::clone(ns))
        } else {
            debug!("no namespaces associated with the current controller");
            None
        }
    }

    /// register the controller as an io device
    fn register_io_device(&self) {
        unsafe {
            spdk_io_device_register(
                self.id() as *mut c_void,
                Some(NvmeControllerIoChannel::create),
                Some(NvmeControllerIoChannel::destroy),
                std::mem::size_of::<NvmeIoChannel>() as u32,
                self.get_name().as_ptr() as *const i8,
            )
        }

        debug!(
            "{}: I/O device registered at 0x{:X}",
            self.get_name(),
            self.id()
        );
    }

    /// we should try to avoid this
    pub fn ctrlr_as_ptr(&self) -> *mut spdk_nvme_ctrlr {
        self.inner.as_ref().map_or(std::ptr::null_mut(), |c| {
            let ptr = c.ctrlr.as_ptr();
            debug!("SPDK handle {:p}", ptr);
            ptr
        })
    }

    /// populate name spaces, at current we only populate the first namespace
    fn populate_namespaces(&mut self) {
        let ns = unsafe { spdk_nvme_ctrlr_get_ns(self.ctrlr_as_ptr(), 1) };

        if ns.is_null() {
            warn!(
                "{} no namespaces reported by the NVMe controller",
                self.get_name()
            );
        }

        self.inner.as_mut().unwrap().namespaces =
            vec![Arc::new(NvmeNamespace::from_ptr(ns))]
    }

    pub fn reset(
        &mut self,
        cb: IoCompletionCallback,
        cb_arg: *const c_void,
        failover: bool,
    ) -> Result<(), CoreError> {
        info!(
            "{} initiating controller reset, failover = {}",
            self.name, failover
        );

        // Reset can be initiated only via a mutable reference, so we know for
        // sure that the caller is owning the controller exclusively, so
        // we can freely modify controller's state without extra
        // locking.
        match self.state {
            NvmeControllerState::Initializing
            | NvmeControllerState::Destroying
            | NvmeControllerState::Resetting => {
                error!(
                    "{} Controller is in '{:?}' state, reset not possible",
                    self.name, self.state
                );
                return Err(CoreError::ResetDispatch {
                    source: Errno::EBUSY,
                });
            }
            _ => {}
        }

        if failover {
            warn!(
                "{} failover is not supported for controller reset",
                self.name
            );
        }

        let reset_ctx = RESET_CTX_POOL
            .get()
            .unwrap()
            .get(ResetCtx {
                name: self.name.clone(),
                cb,
                cb_arg,
                spdk_handle: self.ctrlr_as_ptr(),
            })
            .ok_or(CoreError::ResetDispatch {
                source: Errno::ENOMEM,
            })?;

        // Mark controller as being under reset and schedule asynchronous reset.
        self.set_state(NvmeControllerState::Resetting);

        unsafe {
            spdk_for_each_channel(
                self.id as *mut c_void,
                Some(NvmeController::reset_destroy_channels),
                reset_ctx as *mut c_void,
                Some(NvmeController::reset_destroy_channels_done),
            );
        }
        Ok(())
    }

    fn complete_reset(reset_ctx: &ResetCtx, status: i32) {
        // Set controller state to Running and invoke completion callback.
        let c = NVME_CONTROLLERS
            .lookup_by_name(&reset_ctx.name)
            .expect("Controller was removed while reset is in progress");
        let mut controller = c.lock().expect("lock poisoned");

        controller.set_state(NvmeControllerState::Running);
        // Unlock the controller before calling the callback to avoid potential
        // deadlocks.
        drop(controller);

        (reset_ctx.cb)(status == 0, reset_ctx.cb_arg);
    }

    extern "C" fn reset_destroy_channels(i: *mut spdk_io_channel_iter) {
        let ch = unsafe { spdk_io_channel_iter_get_channel(i) };
        let inner = NvmeIoChannel::inner_from_channel(ch);

        debug!("Resetting I/O channel");
        let rc = inner.reset();
        if rc == 0 {
            debug!("I/O channel successfully reset");
        } else {
            error!("failed to reset I/O channel, reset aborted");
        }

        unsafe { spdk_for_each_channel_continue(i, rc) };
    }

    extern "C" fn reset_destroy_channels_done(
        i: *mut spdk_io_channel_iter,
        status: i32,
    ) {
        unsafe {
            let reset_ctx = spdk_io_channel_iter_get_ctx(i) as *mut ResetCtx;

            if status != 0 {
                error!(
                    "{}: controller reset failed with status = {}",
                    (*reset_ctx).name,
                    status
                );
                NvmeController::complete_reset(&*reset_ctx, status);
                return;
            }

            info!("{} all qpairs successfully deallocated", (*reset_ctx).name);

            let rc = spdk_nvme_ctrlr_reset((*reset_ctx).spdk_handle);
            if rc != 0 {
                error!(
                    "{} failed to reset controller, rc = {}",
                    (*reset_ctx).name,
                    rc
                );
                NvmeController::complete_reset(&*reset_ctx, rc);
            } else {
                info!("{} controller successfully reset", (*reset_ctx).name);

                /* Recreate all of the I/O queue pairs */
                spdk_for_each_channel(
                    spdk_io_channel_iter_get_io_device(i),
                    Some(NvmeController::reset_create_channels),
                    spdk_io_channel_iter_get_ctx(i),
                    Some(NvmeController::reset_create_channels_done),
                );
            }
        }
    }

    extern "C" fn reset_create_channels(i: *mut spdk_io_channel_iter) {
        let reset_ctx =
            unsafe { spdk_io_channel_iter_get_ctx(i) as *mut ResetCtx };
        let ch = unsafe { spdk_io_channel_iter_get_channel(i) };
        let inner = NvmeIoChannel::inner_from_channel(ch);

        debug!("Reinitializing I/O channel");
        unsafe {
            let rc = inner
                .reinitialize(&(*reset_ctx).name, (*reset_ctx).spdk_handle);
            if rc != 0 {
                error!(
                    "{} failed to reinitialize I/O channel, rc = {}",
                    (*reset_ctx).name,
                    rc
                );
            } else {
                info!(
                    "{} I/O channel successfully reinitialized",
                    (*reset_ctx).name
                );
            }

            spdk_for_each_channel_continue(i, rc)
        }
    }

    extern "C" fn reset_create_channels_done(
        i: *mut spdk_io_channel_iter,
        status: i32,
    ) {
        unsafe {
            let reset_ctx = spdk_io_channel_iter_get_ctx(i) as *mut ResetCtx;

            info!(
                "{} controller reset completed, status = {}",
                (*reset_ctx).name,
                status
            );
            NvmeController::complete_reset(&*reset_ctx, status);
        }
    }
}

impl<'a> Drop for NvmeController<'a> {
    fn drop(&mut self) {
        let inner = self.inner.take().expect("NVMe inner already gone");
        inner.adminq_poller.stop();

        debug!(
            "{}: unregistering I/O device at 0x{:X}",
            self.get_name(),
            self.id()
        );
        unsafe {
            spdk_io_device_unregister(self.id() as *mut c_void, None);
        }
        let rc = unsafe { spdk_nvme_detach(inner.ctrlr.as_ptr()) };

        assert_eq!(rc, 0, "Failed to detach NVMe controller");
        debug!("{}: NVMe controller successfully detached", self.name);
    }
}

/// return number of completions processed (maybe 0) or negated on error. -ENXIO
//  in the special case that the qpair is failed at the transport layer.
pub extern "C" fn nvme_poll_adminq(ctx: *mut c_void) -> i32 {
    //println!("adminq poll");

    let rc = unsafe {
        spdk_nvme_ctrlr_process_admin_completions(ctx as *mut spdk_nvme_ctrlr)
    };

    if rc == 0 {
        0
    } else {
        1
    }
}

pub(crate) fn connected_attached_cb(
    ctx: &mut NvmeControllerContext,
    ctrlr: NonNull<spdk_nvme_ctrlr>,
) {
    ctx.unregister_poller();
    // we use the ctrlr address as the controller id in the global table
    let cid = ctrlr.as_ptr() as u64;

    // get a reference to our controller we created when we kicked of the async
    // attaching process
    let controller = NVME_CONTROLLERS
        .lookup_by_name(&ctx.name())
        .expect("no controller in the list");

    // clone it now such that we can lock the original, and insert it later.
    let ctl = Arc::clone(&controller);

    let mut controller = controller.lock().unwrap();

    controller.set_id(cid);
    controller.inner = Some(NvmeControllerInner::new(ctrlr));
    controller.register_io_device();

    debug!(
        "{}: I/O device registered at 0x{:X}",
        controller.get_name(),
        controller.id()
    );

    controller.populate_namespaces();
    controller.state = NvmeControllerState::Running;

    // Proactively initialize cache for controller operations.
    RESET_CTX_POOL.get_or_init(|| {
        MemoryPool::<ResetCtx>::create(
            "nvme_ctrlr_reset_ctx",
            RESET_CTX_POOL_SIZE,
        )
        .expect(
            "Failed to create memory pool for NVMe controller reset contexts",
        )
    });

    NVME_CONTROLLERS.insert_controller(cid.to_string(), ctl);

    // Wake up the waiter and complete controller registration.
    ctx.sender()
        .send(Ok(()))
        .expect("done callback receiver side disappeared");
}

pub(crate) mod options {
    use std::mem::size_of;

    use spdk_sys::{
        spdk_nvme_ctrlr_get_default_ctrlr_opts,
        spdk_nvme_ctrlr_opts,
    };

    /// structure that holds the default NVMe controller options. This is
    /// different from ['NvmeBdevOpts'] as it exposes more control over
    /// variables.

    pub struct NvmeControllerOpts(spdk_nvme_ctrlr_opts);
    impl NvmeControllerOpts {
        pub fn as_ptr(&self) -> *const spdk_nvme_ctrlr_opts {
            &self.0
        }
    }

    impl Default for NvmeControllerOpts {
        fn default() -> Self {
            let mut default = spdk_nvme_ctrlr_opts::default();
            unsafe {
                spdk_nvme_ctrlr_get_default_ctrlr_opts(
                    &mut default,
                    size_of::<spdk_nvme_ctrlr_opts>() as u64,
                );
            }

            Self(default)
        }
    }

    #[derive(Debug, Default)]
    pub struct Builder {
        admin_timeout_ms: Option<u32>,
        disable_error_logging: Option<bool>,
        fabrics_connect_timeout_us: Option<u64>,
        transport_retry_count: Option<u8>,
        keep_alive_timeout_ms: Option<u32>,
    }

    #[allow(dead_code)]
    impl Builder {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_admin_timeout_ms(mut self, timeout: u32) -> Self {
            self.admin_timeout_ms = Some(timeout);
            self
        }
        pub fn with_fabrics_connect_timeout_us(mut self, timeout: u64) -> Self {
            self.fabrics_connect_timeout_us = Some(timeout);
            self
        }

        pub fn with_transport_retry_count(mut self, count: u8) -> Self {
            self.transport_retry_count = Some(count);
            self
        }

        pub fn with_keep_alive_timeout_ms(mut self, timeout: u32) -> Self {
            self.keep_alive_timeout_ms = Some(timeout);
            self
        }

        pub fn disable_error_logging(mut self, disable: bool) -> Self {
            self.disable_error_logging = Some(disable);
            self
        }

        /// Builder to override default values
        pub fn build(self) -> NvmeControllerOpts {
            let mut opts = NvmeControllerOpts::default();

            if let Some(timeout_ms) = self.admin_timeout_ms {
                opts.0.admin_timeout_ms = timeout_ms;
            }
            if let Some(timeout_us) = self.fabrics_connect_timeout_us {
                opts.0.fabrics_connect_timeout_us = timeout_us;
            }

            if let Some(retries) = self.transport_retry_count {
                opts.0.transport_retry_count = retries;
            }

            if let Some(timeout_ms) = self.keep_alive_timeout_ms {
                opts.0.keep_alive_timeout_ms = timeout_ms;
            }

            opts
        }
    }
    #[cfg(test)]
    mod test {
        use crate::bdev::dev::nvmx::controller::options;

        #[test]
        fn nvme_default_controller_options() {
            let opts = options::Builder::new()
                .with_admin_timeout_ms(1)
                .with_fabrics_connect_timeout_us(1)
                .with_transport_retry_count(1)
                .build();

            assert_eq!(opts.0.admin_timeout_ms, 1);
            assert_eq!(opts.0.fabrics_connect_timeout_us, 1);
            assert_eq!(opts.0.transport_retry_count, 1);
        }
    }
}

pub(crate) mod transport {
    use libc::c_void;
    use spdk_sys::spdk_nvme_transport_id;
    use std::{ffi::CStr, fmt::Debug, ptr::copy_nonoverlapping};

    pub struct NvmeTransportId(spdk_nvme_transport_id);

    impl Debug for NvmeTransportId {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            writeln!(
                f,
                "Transport ID: {}: {}: {}: {}:",
                self.trtype(),
                self.traddr(),
                self.subnqn(),
                self.svcid()
            )
        }
    }

    impl NvmeTransportId {
        pub fn trtype(&self) -> String {
            unsafe {
                CStr::from_ptr(&self.0.trstring[0])
                    .to_string_lossy()
                    .to_string()
            }
        }

        pub fn traddr(&self) -> String {
            unsafe {
                CStr::from_ptr(&self.0.traddr[0])
                    .to_string_lossy()
                    .to_string()
            }
        }

        pub fn subnqn(&self) -> String {
            unsafe {
                CStr::from_ptr(&self.0.subnqn[0])
                    .to_string_lossy()
                    .to_string()
            }
        }
        pub fn svcid(&self) -> String {
            unsafe {
                CStr::from_ptr(&self.0.trsvcid[0])
                    .to_string_lossy()
                    .to_string()
            }
        }

        pub fn as_ptr(&self) -> *const spdk_nvme_transport_id {
            &self.0
        }
    }

    #[derive(Debug)]
    enum TransportId {
        TCP = 0x3,
    }

    impl Default for TransportId {
        fn default() -> Self {
            Self::TCP
        }
    }

    impl From<TransportId> for String {
        fn from(t: TransportId) -> Self {
            match t {
                TransportId::TCP => String::from("tcp"),
            }
        }
    }

    #[derive(Debug)]
    #[allow(dead_code)]
    pub(crate) enum AdressFamily {
        NvmfAdrfamIpv4 = 0x1,
        NvmfAdrfamIpv6 = 0x2,
        NvmfAdrfamIb = 0x3,
        NvmfAdrfamFc = 0x4,
        NvmfAdrfamLoop = 0xfe,
    }

    impl Default for AdressFamily {
        fn default() -> Self {
            Self::NvmfAdrfamIpv4
        }
    }

    #[derive(Default, Debug)]
    pub struct Builder {
        trid: TransportId,
        adrfam: AdressFamily,
        svcid: String,
        traddr: String,
        subnqn: String,
    }

    impl Builder {
        pub fn new() -> Self {
            Self {
                ..Default::default()
            }
        }

        /// the address to connect to
        pub fn with_traddr(mut self, traddr: &str) -> Self {
            self.traddr = traddr.to_string();
            self
        }
        /// svcid (port) to connect to

        pub fn with_svcid(mut self, svcid: &str) -> Self {
            self.svcid = svcid.to_string();
            self
        }

        /// target nqn
        pub fn with_subnqn(mut self, subnqn: &str) -> Self {
            self.subnqn = subnqn.to_string();
            self
        }

        /// builder for transportID currently defaults to TCP IPv4
        pub fn build(self) -> NvmeTransportId {
            let trtype = String::from(TransportId::TCP);
            let mut trid = spdk_nvme_transport_id {
                adrfam: AdressFamily::NvmfAdrfamIpv4 as u32,
                trtype: TransportId::TCP as u32,
                ..Default::default()
            };

            unsafe {
                copy_nonoverlapping(
                    trtype.as_ptr().cast(),
                    &mut trid.trstring[0] as *const _ as *mut c_void,
                    trtype.len(),
                );

                copy_nonoverlapping(
                    self.traddr.as_ptr().cast(),
                    &mut trid.traddr[0] as *const _ as *mut c_void,
                    self.traddr.len(),
                );
                copy_nonoverlapping(
                    self.svcid.as_ptr() as *const c_void,
                    &mut trid.trsvcid[0] as *const _ as *mut c_void,
                    self.svcid.len(),
                );
                copy_nonoverlapping(
                    self.subnqn.as_ptr() as *const c_void,
                    &mut trid.subnqn[0] as *const _ as *mut c_void,
                    self.subnqn.len(),
                );
            };

            NvmeTransportId(trid)
        }
    }

    #[cfg(test)]
    mod test {
        use crate::bdev::dev::nvmx::controller::transport;

        #[test]
        fn test_transport_id() {
            let transport = transport::Builder::new()
                .with_subnqn("nqn.2021-01-01:test.nqn")
                .with_svcid("4420")
                .with_traddr("127.0.0.1")
                .build();

            assert_eq!(transport.traddr(), "127.0.0.1");
            assert_eq!(transport.subnqn(), "nqn.2021-01-01:test.nqn");
            assert_eq!(transport.svcid(), "4420");
        }
    }
}