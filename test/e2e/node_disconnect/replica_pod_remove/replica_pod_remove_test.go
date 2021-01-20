package replica_pod_remove_test

import (
	"e2e-basic/common"
	disconnect_lib "e2e-basic/node_disconnect/lib"

	logf "sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"

	"os"
	"testing"

	. "github.com/onsi/ginkgo"
	"github.com/onsi/ginkgo/reporters"
	. "github.com/onsi/gomega"
)

var env disconnect_lib.DisconnectEnv

const gStorageClass = "mayastor-nvmf-pod-remove-test-sc"

func TestMayastorPodLoss(t *testing.T) {
	RegisterFailHandler(Fail)
	reportDir := os.Getenv("e2e_reports_dir")
	junitReporter := reporters.NewJUnitReporter(reportDir + "/replica-pod-remove-junit.xml")
	RunSpecsWithDefaultAndCustomReporters(t, "Replica pod removal tests",
		[]Reporter{junitReporter})
}

var _ = Describe("Mayastor replica pod removal test", func() {
	It("should define the storage class to use", func() {
		common.MkStorageClass(gStorageClass, 2, "nvmf", "io.openebs.csi-mayastor")
	})

	It("should verify nvmf nexus behaviour when a mayastor pod is removed", func() {
		env = disconnect_lib.Setup("loss-test-pvc-nvmf", gStorageClass, "fio-pod-remove-test")
		env.PodLossTest()
	})
})

var _ = BeforeSuite(func(done Done) {
	logf.SetLogger(zap.New(zap.UseDevMode(true), zap.WriteTo(GinkgoWriter)))
	common.SetupTestEnv()
	close(done)
}, 60)

var _ = AfterSuite(func() {
	By("tearing down the test environment")

	env.UnsuppressMayastorPod()
	env.Teardown() // removes fio pod and volume

	common.RmStorageClass(gStorageClass)
	common.TeardownTestEnv()
})
