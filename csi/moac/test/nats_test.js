// Unit tests for the nats message bus

'use strict';

const expect = require('chai').expect;
const { spawn } = require('child_process');
const nats = require('nats');
const sleep = require('sleep-promise');
const Registry = require('../registry');
const { MessageBus } = require('../nats');
const { waitUntil } = require('./utils');
const NodeStub = require('./node_stub');

const NATS_PORT = '14222';
const NATS_HOST = '127.0.0.1';
const NATS_EP = `${NATS_HOST}:${NATS_PORT}`;
const RECONNECT_DELAY = 300;
const GRPC_ENDPOINT = '127.0.0.1:12345';
const NODE_NAME = 'node-name';

var natsProc;

// Starts nats server and call callback when the server is up and ready.
function startNats (done) {
  natsProc = spawn('nats-server', ['-a', NATS_HOST, '-p', NATS_PORT]);
  var doneCalled = false;
  var stderr = '';

  natsProc.stderr.on('data', (data) => {
    stderr += data.toString();
    if (data.toString().match(/Server is ready/)) {
      doneCalled = true;
      done();
    }
  });

  natsProc.once('close', (code) => {
    natsProc = null;
    if (!doneCalled) {
      if (code) {
        done(new Error(`nats server exited with code ${code}: ${stderr}`));
      } else {
        done(new Error('nats server exited prematurely'));
      }
      return;
    }
    if (code) {
      console.log(`nats server exited with code ${code}: ${stderr}`);
    }
  });
}

// Kill nats server. Though it does not wait for it to exit!
function stopNats () {
  if (natsProc) natsProc.kill();
}

module.exports = function () {
  var eventBus;
  var registry;
  var nc;

  // Create registry, event bus object, nats client and start nat server
  before((done) => {
    registry = new Registry();
    registry.Node = NodeStub;
    eventBus = new MessageBus(registry, RECONNECT_DELAY);
    startNats(err => {
      if (err) return done(err);
      nc = nats.connect(`nats://${NATS_EP}`);
      nc.on('connect', () => done());
    });
  });

  after(() => {
    eventBus.stop();
    if (nc) {
      nc.close();
      nc = null;
    }
    stopNats();
  });

  it('should connect to the nats server', async () => {
    eventBus.start(NATS_EP);

    await waitUntil(async () => {
      return eventBus.isConnected();
    }, 1000, 'connect to NATS');
  });

  it('should register a node', async () => {
    nc.publish('register', JSON.stringify({
      id: NODE_NAME,
      grpcEndpoint: GRPC_ENDPOINT
    }));
    await waitUntil(async () => {
      return registry.getNode(NODE_NAME);
    }, 1000, 'new node');
    const node = registry.getNode(NODE_NAME);
    expect(node.name).to.equal(NODE_NAME);
    expect(node.endpoint).to.equal(GRPC_ENDPOINT);
  });

  it('should ignore register request with missing node name', async () => {
    nc.publish('register', JSON.stringify({
      grpcEndpoint: GRPC_ENDPOINT
    }));
    // small delay to wait for a possible crash of moac
    await sleep(10);
  });

  it('should ignore register request with missing grpc endpoint', async () => {
    nc.publish('register', JSON.stringify({
      id: NODE_NAME
    }));
    // small delay to wait for a possible crash of moac
    await sleep(10);
  });

  it('should not crash upon a request with invalid JSON', async () => {
    nc.publish('register', '{"id": "NODE", "grpcEndpoint": "something"');
    // small delay to wait for a possible crash of moac
    await sleep(10);
  });

  it('should deregister a node', async () => {
    nc.publish('deregister', JSON.stringify({
      id: NODE_NAME
    }));
    await waitUntil(async () => {
      return !registry.getNode(NODE_NAME);
    }, 1000, 'node removal');
  });

  it('should disconnect from the nats server', () => {
    eventBus.stop();
    expect(eventBus.isConnected()).to.be.false();
  });

  it('should retry connect until successfull', async () => {
    stopNats();
    await sleep(100);
    eventBus.start(NATS_EP);
    await sleep(500);

    let resolveCb, rejectCb;
    const NatsStarted = new Promise((resolve, reject) => {
      resolveCb = resolve;
      rejectCb = reject;
    });
    startNats((err) => {
      if (err) rejectCb(err);
      else resolveCb();
    });
    await NatsStarted;
    await waitUntil(async () => {
      return eventBus.isConnected();
    }, 1000, 'connect to NATS');
  });
};
