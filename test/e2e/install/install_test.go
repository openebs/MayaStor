package basic_test

import (
	"e2e-basic/common"
	"e2e-basic/common/e2e_config"
	rep "e2e-basic/common/reporter"

	"fmt"
	"os/exec"
	"path"
	"runtime"
	"testing"

	. "github.com/onsi/ginkgo"
	. "github.com/onsi/gomega"

	logf "sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"
)

const scriptsDir = "../../../scripts"
const yamlsDir = "../../../artifacts/test-yamls"

// Encapsulate the logic to find where the deploy yamls are
func getDeployYamlDir() string {
	_, filename, _, _ := runtime.Caller(0)
	return path.Clean(filename + "/../../../../deploy")
}

// Create mayastor namespace
func createNamespace() {
	cmd := exec.Command("kubectl", "create", "namespace", common.NSMayastor)
	out, err := cmd.CombinedOutput()
	Expect(err).ToNot(HaveOccurred(), "%s", out)
}

// Helper for passing yaml from the deploy directory to kubectl
func applyDeployYaml(filename string) {
	cmd := exec.Command("kubectl", "apply", "-f", filename)
	cmd.Dir = getDeployYamlDir()
	out, err := cmd.CombinedOutput()
	Expect(err).ToNot(HaveOccurred(), "%s", out)
}

// Helper for passing yaml from the generated directory to kubectl
func applyGeneratedYaml(filename string) {
	cmd := exec.Command("kubectl", "apply", "-f", yamlsDir+"/"+filename)
	out, err := cmd.CombinedOutput()
	Expect(err).ToNot(HaveOccurred(), "%s", out)
}

func generateYamlFiles(imageTag string, registryAddress string, mayastorNodes []string, e2eCfg *e2e_config.E2EConfig) {
	coresDirective := ""
	if e2eCfg.Cores != 0 {
		coresDirective = fmt.Sprintf("%s -c %d", coresDirective, e2eCfg.Cores)
	}

	poolDirectives := ""
	if len(e2eCfg.PoolDevice) != 0 {
		poolDevice := e2eCfg.PoolDevice
		for _, mayastorNode := range mayastorNodes {
			poolDirectives += fmt.Sprintf(" -p '%s,%s'", mayastorNode, poolDevice)
		}
	}

	bashCmd := fmt.Sprintf(
		"%s/generate-deploy-yamls.sh -o %s -t '%s' -r '%s' %s %s test",
		scriptsDir, yamlsDir, imageTag, registryAddress, coresDirective, poolDirectives,
	)
	cmd := exec.Command("bash", "-c", bashCmd)
	out, err := cmd.CombinedOutput()
	Expect(err).ToNot(HaveOccurred(), "%s", out)
}

// create pools for the cluster
//
// TODO: Ideally there should be one way how to create pools without using
// two env variables to do a similar thing.
func createPools(mayastorNodes []string, e2eCfg *e2e_config.E2EConfig) {
	// TODO: It is an error if configuration specifies both pool devices and pool definition yaml files,
	// as this simple code does not resolve that case in an obvious or defined way.

	poolYamlFiles := e2eCfg.PoolYamlFiles
	poolDevice := e2eCfg.PoolDevice
	if len(poolYamlFiles) != 0 {
		// Apply the list of externally defined pool yaml files
		// NO check is made on the status of pools
		for _, poolYaml := range poolYamlFiles {
			fmt.Println("applying ", poolYaml)
			bashCmd := "kubectl apply -f " + poolYaml
			cmd := exec.Command("bash", "-c", bashCmd)
			_, err := cmd.CombinedOutput()
			Expect(err).ToNot(HaveOccurred(), "Applying %s failed", poolYaml)
		}
	} else if len(poolDevice) != 0 {
		// Use the generated file to create pools as per the devices
		// NO check is made on the status of pools
		poolYaml := yamlsDir + "/pool.yaml"
		bashCmd := "kubectl apply -f " + poolYaml
		cmd := exec.Command("bash", "-c", bashCmd)
		_, err := cmd.CombinedOutput()
		Expect(err).ToNot(HaveOccurred(), "Applying %s failed", poolYaml)
	} else {
		Expect(false).To(BeTrue(), "Neither e2e_pool_yaml_files nor e2e_pool_device specified")
	}
}

// Install mayastor on the cluster under test.
// We deliberately call out to kubectl, rather than constructing the client-go
// objects, so that we can verify the local deploy yaml files are correct.
func installMayastor() {
	e2eCfg := e2e_config.GetConfig()

	Expect(e2eCfg.ImageTag).ToNot(BeEmpty(),
		"mayastor image tag not defined")
	Expect(e2eCfg.Registry).ToNot(BeEmpty(),
		"registry not defined")
	Expect(e2eCfg.PoolDevice != "" || len(e2eCfg.PoolYamlFiles) != 0).To(BeTrue(),
		"configuration error pools are not defined.")
	Expect(e2eCfg.PoolDevice == "" || len(e2eCfg.PoolYamlFiles) == 0).To(BeTrue(),
		"Unable to resolve pool definitions if both pool device and pool yaml files are defined")

	imageTag := e2eCfg.ImageTag
	registry := e2eCfg.Registry

	nodes, err := common.GetNodeLocs()
	Expect(err).ToNot(HaveOccurred())

	var mayastorNodes []string
	numMayastorInstances := 0

	for _, node := range nodes {
		if node.MayastorNode && !node.MasterNode {
			mayastorNodes = append(mayastorNodes, node.NodeName)
			numMayastorInstances += 1
		}
	}
	Expect(numMayastorInstances).ToNot(Equal(0))

	fmt.Printf("tag %v, registry %v, # of mayastor instances=%v\n", imageTag, registry, numMayastorInstances)

	// FIXME use absolute paths, do not depend on CWD
	createNamespace()
	generateYamlFiles(imageTag, registry, mayastorNodes, &e2eCfg)
	applyDeployYaml("storage-class.yaml")
	applyGeneratedYaml("moac-rbac.yaml")
	applyDeployYaml("mayastorpoolcrd.yaml")
	applyGeneratedYaml("nats-deployment.yaml")
	applyGeneratedYaml("csi-daemonset.yaml")
	applyGeneratedYaml("moac-deployment.yaml")
	applyGeneratedYaml("mayastor-daemonset.yaml")

	ready, err := common.MayastorReady(2, 540)
	Expect(err).ToNot(HaveOccurred())
	Expect(ready).To(BeTrue())

	// Now create pools on all nodes.
	createPools(mayastorNodes, &e2eCfg)

	// Mayastor has been installed and is now ready for use.
}

func TestInstallSuite(t *testing.T) {
	RegisterFailHandler(Fail)
	RunSpecsWithDefaultAndCustomReporters(t, "Basic Install Suite", rep.GetReporters("install"))
}

var _ = Describe("Mayastor setup", func() {
	It("should install using yaml files", func() {
		installMayastor()
	})
})

var _ = BeforeSuite(func(done Done) {
	logf.SetLogger(zap.New(zap.UseDevMode(true), zap.WriteTo(GinkgoWriter)))
	common.SetupTestEnv()

	close(done)
}, 60)

var _ = AfterSuite(func() {
	By("tearing down the test environment")
	common.TeardownTestEnvNoCleanup()
})
