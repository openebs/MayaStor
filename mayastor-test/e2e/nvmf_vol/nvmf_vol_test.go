package nvmf_vol_test

import (
	"fmt"
	"testing"

	"e2e-basic/common"

	. "github.com/onsi/ginkgo"
	. "github.com/onsi/gomega"
	logf "sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"
)

var defTimeoutSecs = "90s"

var g_environment common.TestEnvironment

func nvmfTest() {
	fmt.Printf("running fio\n")
	common.RunFio()
}

func TestNvmfVol(t *testing.T) {
	RegisterFailHandler(Fail)
	RunSpecs(t, "Node Loss Test Suite")
}

var _ = Describe("Mayastor nvmf IO test", func() {
	It("should verify an nvmf volume can process IO", func() {
		nvmfTest()
	})
})

var _ = BeforeSuite(func(done Done) {

	logf.SetLogger(zap.LoggerTo(GinkgoWriter, true))

	g_environment = common.SetupTestEnv()

	common.MkPVC(fmt.Sprintf("vol-test-pvc-nvmf"), "mayastor-nvmf", &g_environment.DynamicClient, &g_environment.KubeInt)
	common.ApplyDeployYaml("deploy/fio_nvmf.yaml")

	fmt.Printf("waiting for fio\n")
	Eventually(func() bool {
		return common.FioReadyPod(&g_environment.K8sClient)
	},
		defTimeoutSecs, // timeout
		"1s",           // polling interval
	).Should(Equal(true))

	close(done)
}, 60)

var _ = AfterSuite(func() {
	// NB This only tears down the local structures for talking to the cluster,
	// not the kubernetes cluster itself.
	By("tearing down the test environment")

	fmt.Printf("removing fio pod\n")
	common.DeleteDeployYaml("deploy/fio_nvmf.yaml")

	fmt.Printf("removing pvc\n")
	common.RmPVC(fmt.Sprintf("vol-test-pvc-nvmf"), "mayastor-nvmf", &g_environment.DynamicClient, &g_environment.KubeInt)

	common.TeardownTestEnv(&g_environment)
})
