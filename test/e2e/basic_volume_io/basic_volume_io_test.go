// JIRA: CAS-505
// JIRA: CAS-506
package basic_volume_io_test

import (
	"e2e-basic/common"
	"e2e-basic/common/e2e_config"
	"e2e-basic/common/loki"
	rep "e2e-basic/common/reporter"
	"testing"

	. "github.com/onsi/ginkgo"
	. "github.com/onsi/gomega"
	logf "sigs.k8s.io/controller-runtime/pkg/log"
)

var defTimeoutSecs = "120s"

type volSc struct {
	volName string
	scName  string
}

var podNames []string
var volNames []volSc

func TestBasicVolumeIO(t *testing.T) {
	RegisterFailHandler(Fail)
	RunSpecsWithDefaultAndCustomReporters(t, "Basic volume IO tests, NVMe-oF TCP and iSCSI", rep.GetReporters("basic-volume-io"))
}

func basicVolumeIOTest(protocol common.ShareProto) {
	err := loki.SendLokiMarker("Starting basic vol IO test")
	Expect(err).ToNot(HaveOccurred())

	scName := "basic-vol-io-test-" + string(protocol)
	err = common.MkStorageClass(scName, e2e_config.GetConfig().BasicVolumeIO.Replicas, protocol)
	Expect(err).ToNot(HaveOccurred(), "Creating storage class %s", scName)

	volName := "basic-vol-io-test-" + string(protocol)
	// Create the volume
	common.MkPVC(volName, scName)
	tmp := volSc{volName, scName}
	volNames = append(volNames, tmp)

	// Create the fio Pod
	fioPodName := "fio-" + volName
	pod, err := common.CreateFioPod(fioPodName, volName)
	Expect(err).ToNot(HaveOccurred())
	Expect(pod).ToNot(BeNil())
	podNames = append(podNames, fioPodName)

	// Wait for the fio Pod to transition to running
	Eventually(func() bool {
		return common.IsPodRunning(fioPodName)
	},
		defTimeoutSecs,
		"1s",
	).Should(Equal(true))

	// Run the fio test
	_, err = common.RunFio(fioPodName, 20, common.FioFsFilename)
	Expect(err).ToNot(HaveOccurred())

	podNames = podNames[:len(podNames)-1]

	// Delete the fio pod
	err = common.DeletePod(fioPodName)
	Expect(err).ToNot(HaveOccurred())

	// Delete the volume
	common.RmPVC(volName, scName)
	volNames = volNames[:len(volNames)-1]

	err = common.RmStorageClass(scName)
	Expect(err).ToNot(HaveOccurred(), "Deleting storage class %s", scName)
}

var _ = Describe("Mayastor Volume IO test", func() {

	AfterEach(func() {
		logf.Log.Info("AfterEach")

		// Check resource leakage.
		err := common.AfterEachCheck()
		Expect(err).ToNot(HaveOccurred())
	})

	It("should verify an NVMe-oF TCP volume can process IO", func() {
		basicVolumeIOTest(common.ShareProtoNvmf)
	})
	It("should verify an iSCSI volume can process IO", func() {
		basicVolumeIOTest(common.ShareProtoIscsi)
	})
})

var _ = BeforeSuite(func(done Done) {
	common.SetupTestEnv()

	close(done)
}, 60)

var _ = AfterSuite(func() {
	// NB This only tears down the local structures for talking to the cluster,
	// not the kubernetes cluster itself.	By("tearing down the test environment")
	common.TeardownTestEnv()
})
