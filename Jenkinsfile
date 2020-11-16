#!/usr/bin/env groovy

// Will ABORT current job for cases when we don't want to build
if (currentBuild.getBuildCauses('jenkins.branch.BranchIndexingCause') &&
    BRANCH_NAME == "develop") {
    print "INFO: Branch Indexing, aborting job."
    currentBuild.result = 'ABORTED'
    return
}

// Update status of a commit in github
def updateGithubCommitStatus(commit, msg, state) {
  step([
    $class: 'GitHubCommitStatusSetter',
    reposSource: [$class: "ManuallyEnteredRepositorySource", url: "https://github.com/openebs/Mayastor.git"],
    commitShaSource: [$class: "ManuallyEnteredShaSource", sha: commit],
    errorHandlers: [[$class: "ChangingBuildStatusErrorHandler", result: "UNSTABLE"]],
    contextSource: [
      $class: 'ManuallyEnteredCommitContextSource',
      context: 'continuous-integration/jenkins/branch'
    ],
    statusResultSource: [
      $class: 'ConditionalStatusResultSource',
      results: [
        [$class: 'AnyBuildResult', message: msg, state: state]
      ]
    ]
  ])
}

// Searches previous builds to find first non aborted one
def getLastNonAbortedBuild(build) {
  if (build == null) {
    return null;
  }

  if(build.result.toString().equals("ABORTED")) {
    return getLastNonAbortedBuild(build.getPreviousBuild());
  } else {
    return build;
  }
}

// Send out a slack message if branch got broken or has recovered
def notifySlackUponStateChange(build) {
  def cur = build.getResult()
  def prev = getLastNonAbortedBuild(build.getPreviousBuild())?.getResult()
  if (cur != prev) {
    if (cur == 'SUCCESS') {
      slackSend(
        channel: '#mayastor-backend',
        color: 'normal',
        message: "Branch ${env.BRANCH_NAME} has been fixed :beers: (<${env.BUILD_URL}|Open>)"
      )
    } else if (prev == 'SUCCESS') {
      slackSend(
        channel: '#mayastor-backend',
        color: 'danger',
        message: "Branch ${env.BRANCH_NAME} is broken :face_with_raised_eyebrow: (<${env.BUILD_URL}|Open>)"
      )
    }
  }
}

// Only schedule regular builds on develop branch, so we don't need to guard against it
String cron_schedule = BRANCH_NAME == "develop" ? "0 2 * * *" : ""

pipeline {
  agent none
  triggers {
    cron(cron_schedule)
  }

  stages {
    stage('linter') {
      agent { label 'nixos-mayastor' }
      when {
        beforeAgent true
        not {
          anyOf {
            branch 'master'
            branch 'release/*'
          }
        }
      }
      steps {
        updateGithubCommitStatus(env.GIT_COMMIT, 'Started to test the commit', 'pending')
        sh 'nix-shell --run "cargo fmt --all -- --check"'
        sh 'nix-shell --run "cargo clippy --all-targets -- -D warnings"'
        sh 'nix-shell --run "./scripts/js-check.sh"'
      }
    }
    stage('test') {
      when {
        beforeAgent true
        not {
          anyOf {
            branch 'master'
            branch 'release/*'
          }
        }
      }
      parallel {
        stage('rust unit tests') {
          agent { label 'nixos-mayastor' }
          steps {
            sh 'nix-shell --run "./scripts/cargo-test.sh"'
          }
          post {
            always {
              // temporary workaround for leaked spdk_iscsi_conns files
              sh 'sudo rm -f /dev/shm/*'
            }
          }
        }
        stage('mocha api tests') {
          agent { label 'nixos-mayastor' }
          steps {
            sh 'nix-shell --run "./scripts/node-test.sh"'
          }
          post {
            always {
              junit '*-xunit-report.xml'
              // temporary workaround for leaked spdk_iscsi_conns files
              sh 'sudo rm -f /dev/shm/*'
            }
          }
        }
        stage('nix tests') {
          agent { label 'nixos-mayastor-kvm' }
          steps {
            sh 'nix-build ./nix/test -A rebuild'
            sh 'nix-build ./nix/test -A fio_nvme_basic'
            sh 'nix-build ./nix/test -A nvmf_distributed'
            sh 'nix-build ./nix/test -A nvmf_ports'
          }
        }
        stage('moac unit tests') {
          agent { label 'nixos-mayastor' }
          steps {
            sh 'nix-shell --run "./scripts/moac-test.sh"'
          }
          post {
            always {
              junit 'moac-xunit-report.xml'
            }
          }
        }
        stage('dev images') {
          agent { label 'nixos-mayastor' }
          steps {
            sh 'nix-build --no-out-link -A images.mayastor-dev-image'
            sh 'nix-build --no-out-link -A images.mayastor-csi-dev-image'
            sh 'nix-build --no-out-link -A images.moac-image'
            sh 'nix-store --delete /nix/store/*docker-image*'
          }
        }
      }
    }
    stage('push images') {
      agent { label 'nixos-mayastor' }
      when {
        beforeAgent true
        anyOf {
          branch 'master'
          branch 'release/*'
          branch 'develop'
        }
      }
      steps {
        updateGithubCommitStatus(env.GIT_COMMIT, 'Started to test the commit', 'pending')
        withCredentials([usernamePassword(credentialsId: 'dockerhub', usernameVariable: 'USERNAME', passwordVariable: 'PASSWORD')]) {
          sh 'echo $PASSWORD | docker login -u $USERNAME --password-stdin'
        }
        sh './scripts/release.sh'
      }
      post {
        always {
          sh 'docker logout'
          sh 'docker image prune --all --force'
        }
      }
    }
  }

  // The main motivation for post block is that if all stages were skipped
  // (which happens when running cron job and branch != develop) then we don't
  // want to set commit status in github (jenkins will implicitly set it to
  // success).
  post {
    always {
      node(null) {
        script {
          // If no tests were run then we should neither be updating commit
          // status in github nor send any slack messages
          if (currentBuild.result != null) {
            if (currentBuild.getResult() == 'SUCCESS') {
              updateGithubCommitStatus(env.GIT_COMMIT, 'Looks good', 'success')
            } else {
              updateGithubCommitStatus(env.GIT_COMMIT, 'Test failed', 'failure')
            }
            if (env.BRANCH_NAME == 'develop') {
              notifySlackUponStateChange(currentBuild)
            }
          }
        }
      }
    }
  }
}
