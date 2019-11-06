// Common utility functions shared by grpc tests.

'use strict';

const _ = require('lodash');
const assert = require('assert');
const async = require('async');
const find = require('find-process');
const fs = require('fs');
const path = require('path');
const { exec, spawn } = require('child_process');
const sudo = require('./sudo');

const SOCK = '/tmp/mayastor_test.sock';
const CONFIG_PATH = '/tmp/mayastor_test.cfg';
const GRPC_PORT = 10777;
const CSI_ENDPOINT = '/tmp/mayastor_csi_test.sock';
const CSI_ID = 'test-node-id';

var endpoint = '127.0.0.1:' + GRPC_PORT;
var mayastorProc;
var mayastorGrpcProc;
var mayastorOutput = [];
var mayastorGrpcOutput = [];

// Construct path to a rust binary in target/debug/... dir.
function getCmdPath(name) {
  return path.join(__dirname, '..', 'target', 'debug', name);
}

// Run the command as root. We use sudo to gain root privileges.
// If already running with euid = 0, then just spawn the command.
// Return child process handle.
//
// TODO: Beware that glob expansion of file names works differently
// between the two cases. When using just spawn() file names are not
// expanded.
function runAsRoot(cmd, args, nameInPs) {
  let env = _.assignIn({}, process.env, {
    RUST_BACKTRACE: 1,
  });
  if (process.geteuid() === 0) {
    return spawn(cmd, args || [], {
      env,
      shell: true,
    });
  } else {
    return sudo(
      [cmd].concat(args || []),
      {
        spawnOptions: { env },
      },
      nameInPs
    );
  }
}

// Periodically ping mayastor until up and running.
// Ping cb with grpc call is provided by the caller.
function waitFor(ping, done) {
  let last_error;
  let iters = 0;

  async.whilst(
    () => {
      return iters < 10;
    },
    next => {
      iters++;
      ping(err => {
        if (err) {
          last_error = err;
          setTimeout(next, 1000);
        } else {
          last_error = undefined;
          iters = 10;
          next();
        }
      });
    },
    () => {
      done(last_error);
    }
  );
}

// Start mayastor process and wait for them to come up.
function startMayastor(config, done) {
  let args = ['-r', SOCK];

  if (config) {
    fs.writeFileSync(CONFIG_PATH, config);
    args = args.concat(['-c', CONFIG_PATH]);
  }

  mayastorProc = runAsRoot(getCmdPath('mayastor'), args, 'reactor_0');

  mayastorProc.stdout.on('data', data => {
    mayastorOutput.push(data);
  });
  mayastorProc.stderr.on('data', data => {
    mayastorOutput.push(data);
  });
  mayastorProc.once('close', (code, signal) => {
    console.log('mayastor output:');
    console.log('-----------------------------------------------------');
    console.log(mayastorOutput.join('').trim());
    console.log('-----------------------------------------------------');
    mayastorProc = undefined;
    mayastorOutput = [];
  });
  if (done) done();
}

// Start mayastor-agent processes and wait for them to come up.
function startMayastorGrpc(done) {
  mayastorGrpcProc = runAsRoot(getCmdPath('mayastor-agent'), [
    '-v',
    '-n',
    'test-node-id',
    '-a',
    '127.0.0.1',
    '-p',
    GRPC_PORT.toString(),
    '-c',
    CSI_ENDPOINT,
    '-s',
    SOCK,
  ]);

  mayastorGrpcProc.stdout.on('data', data => {
    mayastorGrpcOutput.push(data);
  });
  mayastorGrpcProc.stderr.on('data', data => {
    mayastorGrpcOutput.push(data);
  });
  mayastorGrpcProc.once('close', (code, signal) => {
    console.log('mayastor-agent output:');
    console.log('-----------------------------------------------------');
    console.log(mayastorGrpcOutput.join('').trim());
    console.log('-----------------------------------------------------');
    mayastorGrpcProc = undefined;
    mayastorGrpcOutput = [];
  });
  if (done) done();
}

// Unix domain socket client does not run with root privs (in general) so open
// the socket to everyone.
function fixSocketPerms(done) {
  let child = runAsRoot('chmod', ['a+rw', CSI_ENDPOINT]);
  child.stderr.on('data', data => {
    //console.log('chmod', 'error:', data.toString());
  });
  child.on('close', code => {
    if (code != 0) {
      done('Failed to chmod the socket' + code);
    } else {
      done();
    }
  });
}

function killSudoedProcess(name, pid, done) {
  find('name', name).then(res => {
    var whichPid;
    if (process.geteuid() === 0) {
      whichPid = 'pid';
    } else {
      whichPid = 'ppid';
    }
    res = res.filter(ent => ent[whichPid] == pid);
    if (res.length == 0) {
      return done();
    }
    let child = runAsRoot('kill', ['-s', 'SIGTERM', res[0].pid.toString()]);
    child.stderr.on('data', data => {
      console.log('kill', name, 'error:', data.toString());
    });
    child.on('close', () => {
      done();
    });
  });
}

// Kill mayastor-agent and mayastor processes
function stopMayastor(done) {
  async.parallel(
    [
      async.reflect(cb => {
        if (mayastorGrpcProc) {
          killSudoedProcess('mayastor-agent', mayastorGrpcProc.pid, err => {
            if (err) return cb(err);
            if (mayastorGrpcProc) return mayastorGrpcProc.once('close', cb);
            cb();
          });
        } else {
          cb();
        }
      }),
      async.reflect(cb => {
        if (mayastorProc) {
          try {
            fs.unlinkSync(CONFIG_PATH);
          } catch (err) {}

          killSudoedProcess('mayastor', mayastorProc.pid, err => {
            if (err) return cb(err);
            if (mayastorProc) return mayastorProc.once('close', cb);
            cb();
          });
        } else {
          cb();
        }
      }),
    ],
    (err, results) => {
      done(results[0].error || results[1].error);
    }
  );
}

function restartMayastor(ping, done) {
  assert(mayastorProc);

  async.series(
    [
      next => {
        killSudoedProcess('mayastor', mayastorProc.pid, err => {
          if (err) return next(err);
          if (mayastorProc) return mayastorProc.once('close', next);
          next();
        });
      },
      next => startMayastor(null, next),
      next => waitFor(ping, next),
    ],
    done
  );
}

function restartMayastorGrpc(ping, done) {
  assert(mayastorGrpcProc);

  async.series(
    [
      next => {
        killSudoedProcess('mayastor-agent', mayastorGrpcProc.pid, err => {
          if (err) return next(err);
          if (mayastorGrpcProc) {
            mayastorGrpcProc.once('close', next);
          } else {
            next();
          }
        });
      },
      next => {
        startMayastorGrpc(next);
      },
      next => {
        waitFor(ping, next);
      },
    ],
    done
  );
}

// Execute rpc method using dumb jsonrpc client
function dumbCommand(method, args, done) {
  exec(
    '../target/debug/mctl -s ' +
      SOCK +
      ' raw' +
      ' ' +
      method +
      " '" +
      JSON.stringify(args) +
      "'",
    (err, stdout, stderr) => {
      if (err) {
        done(new Error(stderr));
      } else {
        done();
      }
    }
  );
}

// Ensure that /dev/nbd* devices are writable by the current process.
// If running as root this is a noop.
function ensureNbdWritable(done) {
  if (process.geteuid() != 0) {
    let child = runAsRoot('sh', ['-c', 'chmod o+rw /dev/nbd*']);
    child.stderr.on('data', data => {
      console.log(data.toString());
    });
    child.on('close', (code, signal) => {
      if (code != 0) {
        done(new Error('Failed to chmod nbd devs'));
      } else {
        done();
      }
    });
  } else {
    done();
  }
}

// Undo change to perms of nbd devices done in ensureNbdWritable().
function restoreNbdPerms(done) {
  if (process.geteuid() != 0) {
    let child = runAsRoot('sh', ['-c', 'chmod o-rw /dev/nbd*']);
    child.on('close', (code, signal) => {
      if (code != 0) {
        done(new Error('Failed to chmod nbd devs'));
      } else {
        done();
      }
    });
  } else {
    done();
  }
}

module.exports = {
  CSI_ENDPOINT,
  CSI_ID,
  startMayastor,
  startMayastorGrpc,
  stopMayastor,
  waitFor,
  restartMayastor,
  restartMayastorGrpc,
  fixSocketPerms,
  endpoint,
  dumbCommand,
  runAsRoot,
  ensureNbdWritable,
  restoreNbdPerms,
};
