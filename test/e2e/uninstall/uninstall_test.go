package basic_test

import (
	"e2e-basic/common"
	"e2e-basic/common/e2e_config"
	rep "e2e-basic/common/reporter"

	"os/exec"
	"path"
	"runtime"
	"testing"
	"time"

	. "github.com/onsi/ginkgo"
	. "github.com/onsi/gomega"

	logf "sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"
)

// Encapsulate the logic to find where the deploy yamls are
func getDeployYamlDir() string {
	_, filename, _, _ := runtime.Caller(0)
	return path.Clean(filename + "/../../../../deploy")
}

// Helper for passing yaml from the deploy directory to kubectl
func deleteDeployYaml(filename string) {
	cmd := exec.Command("kubectl", "delete", "-f", filename)
	cmd.Dir = getDeployYamlDir()
	_, err := cmd.CombinedOutput()
	Expect(err).ToNot(HaveOccurred(), "Command failed: kubectl delete -f %s", filename)
}

// Helper for deleting mayastor CRDs
func deleteCRD(crdName string) {
	cmd := exec.Command("kubectl", "delete", "crd", crdName)
	_ = cmd.Run()
}

// Create mayastor namespace
func deleteNamespace() {
	cmd := exec.Command("kubectl", "delete", "namespace", common.NSMayastor)
	out, err := cmd.CombinedOutput()
	Expect(err).ToNot(HaveOccurred(), "%s", out)
}

// Teardown mayastor on the cluster under test.
// We deliberately call out to kubectl, rather than constructing the client-go
// objects, so that we can verify the local deploy yaml files are correct.
func teardownMayastor() {
	var cleaned bool
	cleanup := e2e_config.GetConfig().Uninstall.Cleanup != 0

	logf.Log.Info("Settings:", "cleanup", cleanup)
	if cleanup {
		cleaned = common.CleanUp()
	} else {
		found, err := common.CheckForTestPods()
		if err != nil {
			logf.Log.Info("Failed to checking for test pods.", "error", err)
		} else {
			Expect(found).To(BeFalse(), "Application pods were found, none expected.")
		}

		found, err = common.CheckForPVCs()
		if err != nil {
			logf.Log.Info("Failed to check for PVCs", "error", err)
		}
		Expect(found).To(BeFalse(), "PersistentVolumeClaims were found, none expected.")

		found, err = common.CheckForPVs()
		if err != nil {
			logf.Log.Info("Failed to check PVs", "error", err)
		}
		Expect(found).To(BeFalse(), "PersistentVolumes were found, none expected.")

		found, err = common.CheckForMSVs()
		if err != nil {
			logf.Log.Info("Failed to check MSVs", "error", err)
		}
		Expect(found).To(BeFalse(), "Mayastor volume CRDs were found, none expected.")

	}

	poolsDeleted := common.DeleteAllPools()
	Expect(poolsDeleted).To(BeTrue())

	logf.Log.Info("Cleanup done, Uninstalling mayastor")
	// Deletes can stall indefinitely, try to mitigate this
	// by running the deletes on different threads
	go deleteDeployYaml("csi-daemonset.yaml")
	go deleteDeployYaml("mayastor-daemonset.yaml")
	go deleteDeployYaml("moac-deployment.yaml")
	go deleteDeployYaml("nats-deployment.yaml")

	{
		const timeOutSecs = 240
		const sleepSecs = 10
		maxIters := (timeOutSecs + sleepSecs - 1) / sleepSecs
		numMayastorPods := common.MayastorUndeletedPodCount()
		if numMayastorPods != 0 {
			logf.Log.Info("Waiting for Mayastor pods to be deleted",
				"timeout", timeOutSecs)
		}
		for iter := 0; iter < maxIters && numMayastorPods != 0; iter++ {
			logf.Log.Info("\tWaiting ",
				"seconds", sleepSecs,
				"numMayastorPods", numMayastorPods,
				"iter", iter)
			numMayastorPods = common.MayastorUndeletedPodCount()
			time.Sleep(sleepSecs * time.Second)
		}
	}

	// The focus is on trying to make the cluster reusable, so we try to delete everything.
	// TODO: When we start using a cluster for a single test run  move these set of deletes to after all checks.
	deleteDeployYaml("mayastorpoolcrd.yaml")
	deleteDeployYaml("moac-rbac.yaml")
	deleteDeployYaml("storage-class.yaml")
	deleteCRD("mayastornodes.openebs.io")
	deleteCRD("mayastorvolumes.openebs.io")

	if cleanup {
		// Attempt to forcefully delete mayastor pods
		deleted, podCount, err := common.ForceDeleteMayastorPods()
		Expect(err).ToNot(HaveOccurred(), "ForceDeleteMayastorPods failed %v", err)
		Expect(podCount).To(BeZero(), "All Mayastor pods have not been deleted")
		// Only delete the namespace if there are no pending resources
		// otherwise this hangs.
		deleteNamespace()
		Expect(deleted).To(BeFalse(), "Mayastor pods were force deleted")
		Expect(cleaned).To(BeTrue(), "Application pods or volume resources were deleted")
	} else {
		Expect(common.MayastorUndeletedPodCount()).To(Equal(0), "All Mayastor pods were not removed on uninstall")
		// More verbose here as deleting the namespace is often where this
		// test hangs.
		logf.Log.Info("Deleting the mayastor namespace")
		deleteNamespace()
		logf.Log.Info("Deleted the mayastor namespace")
	}
}

func TestTeardownSuite(t *testing.T) {
	RegisterFailHandler(Fail)
	RunSpecsWithDefaultAndCustomReporters(t, "Basic Teardown Suite", rep.GetReporters("uninstall"))
}

var _ = Describe("Mayastor setup", func() {
	It("should teardown using yamls", func() {
		teardownMayastor()
	})
})

var _ = BeforeSuite(func(done Done) {
	logf.SetLogger(zap.New(zap.UseDevMode(true), zap.WriteTo(GinkgoWriter)))
	common.SetupTestEnv()

	close(done)
}, 60)

var _ = AfterSuite(func() {
	// NB This only tears down the local structures for talking to the cluster,
	// not the kubernetes cluster itself.
	By("tearing down the test environment")
	common.TeardownTestEnv()
})
