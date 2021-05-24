"""Common code that represents a mayastor handle."""
import mayastor_pb2 as pb
import grpc
import mayastor_pb2_grpc as rpc

pytest_plugins = ["docker_compose"]


class MayastorHandle(object):
    """Mayastor gRPC handle."""

    def __init__(self, ip_v4):
        """Init."""
        self.ip_v4 = ip_v4
        self.channel = grpc.insecure_channel(("%s:10124") % self.ip_v4)
        self.bdev = rpc.BdevRpcStub(self.channel)
        self.ms = rpc.MayastorStub(self.channel)
        self.bdev_list()
        self.pool_list()

    def __del__(self):
        del self.channel

    def close(self):
        self.__del__()

    def as_target(self) -> str:
        """Returns this node as scheme which is used to designate this node to
        be used as the node where the nexus shall be created on."""
        node = "nvmt://{0}".format(self.ip_v4)
        return node

    def bdev_create(self, uri):
        """create the bdev using the specific URI where URI can be one of the
        following supported schemes:

            nvmf://
            aio://
            uring://
            malloc://

        Note that we do not check the URI schemes, as this should be done in
        mayastor as this is for testing, we do not want to prevent parsing
        invalid schemes."""

        return self.bdev.Create(pb.BdevUri(uri=uri)).uri

    def pool_create(self, name, bdev):
        """Create a pool with given name on this node using the bdev as the
        backend device. The bdev is implicitly created."""

        disks = []
        disks.append(bdev)
        return self.ms.CreatePool(pb.CreatePoolRequest(name=name, disks=disks))

    def pool_destroy(self, name):
        """Destroy  the pool."""
        return self.ms.DestroyPool(pb.DestroyPoolRequest(name=name))

    def replica_create(self, pool, uuid, size):
        """Create  a replica on the pool with the specified UUID and size."""
        return self.ms.CreateReplica(
            pb.CreateReplicaRequest(
                pool=pool, uuid=str(uuid), size=size, thin=False, share=1
            )
        )

    def replica_destroy(self, uuid):
        """Destroy the replica by the UUID, the pool is resolved within
        mayastor."""
        return self.ms.DestroyReplica(pb.DestroyReplicaRequest(uuid=uuid))

    def nexus_create(self, uuid, size, children):
        """Create a nexus with the given uuid and size. The children are
        should be an array of nvmf URIs."""
        return self.ms.CreateNexus(
            pb.CreateNexusRequest(uuid=str(uuid), size=size, children=children)
        )

    def nexus_destroy(self, uuid):
        """Destroy the nexus."""
        return self.ms.DestroyNexus(pb.DestroyNexusRequest(uuid=uuid))

    def nexus_publish(self, uuid):
        """Publish the nexus. this is the same as bdev_share() but is not used
        by the control plane."""
        return self.ms.PublishNexus(
            pb.PublishNexusRequest(
                uuid=uuid, key="", share=1)).device_uri

    def nexus_unpublish(self, uuid):
        """Unpublish the nexus."""
        return self.ms.UnpublishNexus(pb.UnpublishNexusRequest(uuid=uuid))

    def nexus_list(self):
        """List all the  the nexus devices."""
        return self.ms.ListNexus(pb.Null()).nexus_list

    def bdev_list(self):
        """"List all bdevs found within the system."""
        return self.bdev.List(pb.Null(), wait_for_ready=True)

    def pool_list(self):
        """Only list pools"""
        return self.ms.ListPools(pb.Null(), wait_for_ready=True)

    def pools_as_uris(self):
        """Return a list of pools as found on the system."""
        uris = []
        pools = self.ms.ListPools(pb.Null(), wait_for_ready=True)
        for p in pools.pools:
            uri = "pool://{0}/{1}".format(self.ip_v4, p.name)
            uris.append(uri)
        return uris
