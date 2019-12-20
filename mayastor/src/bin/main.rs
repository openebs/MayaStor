#[macro_use]
extern crate log;

use std::path::Path;

use git_version::git_version;
use mayastor::{
    environment::{args::MayastorCliArgs, env::MayastorEnvironment},
    mayastor_logger_init,
};

use structopt::StructOpt;
use sysfs;

mayastor::CPS_INIT!();

fn main() -> Result<(), std::io::Error> {
    // setup our logger first
    mayastor_logger_init("TRACE");

    let args = MayastorCliArgs::from_args();

    let hugepage_path = Path::new("/sys/kernel/mm/hugepages/hugepages-2048kB");
    let nr_pages: u32 = sysfs::parse_value(&hugepage_path, "nr_hugepages")?;

    if nr_pages == 0 {
        warn!("No hugepages available, trying to allocate 512 2MB hugepages");
        sysfs::write_value(&hugepage_path, "nr_hugepages", 512)?;
    }

    let free_pages: u32 = sysfs::parse_value(&hugepage_path, "free_hugepages")?;
    let nr_pages: u32 = sysfs::parse_value(&hugepage_path, "nr_hugepages")?;

    info!("Starting Mayastor version {}", git_version!());
    info!("free_pages: {} nr_pages: {}", free_pages, nr_pages);
    let _status = MayastorEnvironment::new(args)
        .start(|| {
            info!("Mayastor started {} ({})...", '\u{1F680}', git_version!());
        })
        .unwrap();
    Ok(())
}
