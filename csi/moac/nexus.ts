// Nexus object implementation.

const _ = require('lodash');
const assert = require('assert');
const { GrpcCode, GrpcError, mayastor } = require('./grpc_client');
const log = require('./logger').Logger('nexus');

import { Replica } from './replica';

function compareChildren(a: any, b: any) {
  assert(a.uri);
  assert(b.uri);
  return a.uri.localeCompare(b.uri);
}

export class Nexus {
  node?: any;
  uuid: string;
  size: number;
  deviceUri: string;
  state: string;
  children: any[];

  // Construct new nexus object.
  //
  // @param {object}   props    Nexus properties as obtained from the storage node.
  // @param {string}   props.uuid       ID of the nexus.
  // @param {number}   props.size       Capacity of the nexus in bytes.
  // @param {string}   props.deviceUri  Block device path to the nexus.
  // @param {string}   props.state      State of the nexus.
  // @param {object[]} props.children   Replicas comprising the nexus (uri and state).
  //
  constructor(props: any) {
    this.node = null; // set by registerNexus method on node
    this.uuid = props.uuid;
    this.size = props.size;
    this.deviceUri = props.deviceUri;
    this.state = props.state;
    // children of the nexus (replica URIs and their state)
    this.children = [].concat(props.children || []).sort(compareChildren);
  }

  // Stringify the nexus
  toString() {
    return this.uuid + '@' + (this.node ? this.node.name : 'nowhere');
  }

  // Update object based on fresh properties obtained from mayastor storage node.
  //
  // @param {object}   props            Properties defining the nexus.
  // @param {string}   props.uuid       ID of the nexus.
  // @param {number}   props.size       Capacity of the nexus in bytes.
  // @param {string}   props.deviceUri  Block device URI of the nexus.
  // @param {string}   props.state      State of the nexus.
  // @param {object[]} props.children   Replicas comprising the nexus (uri and state).
  //
  merge(props: any) {
    let changed = false;

    if (this.size !== props.size) {
      this.size = props.size;
      changed = true;
    }
    if (this.deviceUri !== props.deviceUri) {
      this.deviceUri = props.deviceUri;
      changed = true;
    }
    if (this.state !== props.state) {
      this.state = props.state;
      changed = true;
    }
    const children = [].concat(props.children).sort(compareChildren);
    if (!_.isEqual(this.children, children)) {
      this.children = children;
      changed = true;
    }
    if (changed) {
      this._emitMod();
    }
  }

  // When anything in nexus changes, this can be called to emit mod event
  // (a shortcut for frequently used code).
  _emitMod() {
    this.node.emit('nexus', {
      eventType: 'mod',
      object: this
    });
  }

  // Bind nexus to the node.
  //
  // @param {object} node   Node to bind the nexus to.
  //
  bind(node: any) {
    this.node = node;
    log.debug(`Adding "${this.uuid}" to the nexus list of node "${node}"`);
    this.node.emit('nexus', {
      eventType: 'new',
      object: this
    });
  }

  // Unbind the previously bound nexus from the node.
  unbind() {
    log.debug(`Removing "${this}" from the nexus list`);
    this.node.unregisterNexus(this);
    this.node.emit('nexus', {
      eventType: 'del',
      object: this
    });
    this.node = null;
  }

  // Set state of the nexus to offline.
  // This is typically called when mayastor stops running on the node and
  // the pool becomes inaccessible.
  offline() {
    log.warn(`Nexus "${this}" got offline`);
    this.state = 'NEXUS_OFFLINE';
    this._emitMod();
  }

  // Publish the nexus to make accessible for IO.
  // @params {string}   protocol      The nexus share protocol.
  // @returns {string} The device path of nexus block device.
  //
  async publish(protocol: string) {
    var res;

    if (this.deviceUri) {
      throw new GrpcError(
        GrpcCode.ALREADY_EXISTS,
        `Nexus ${this} has been already published`
      );
    }

    const nexusProtocol = 'NEXUS_'.concat(protocol.toUpperCase());
    var share = mayastor.ShareProtocolNexus.type.value.find(
      (ent: any) => ent.name === nexusProtocol
    );
    if (!share) {
      throw new GrpcError(
        GrpcCode.NOT_FOUND,
        `Cannot find protocol "${protocol}" for Nexus ${this}`
      );
    }
    log.info(`Publishing nexus "${this}" with protocol=${protocol} ...`);
    try {
      res = await this.node.call('publishNexus', {
        uuid: this.uuid,
        key: '',
        share: share.number
      });
    } catch (err) {
      throw new GrpcError(
        GrpcCode.INTERNAL,
        `Failed to publish nexus "${this}": ${err}`
      );
    }
    log.info(`Nexus "${this}" is published at "${res.deviceUri}"`);
    this.deviceUri = res.deviceUri;
    this._emitMod();
    return res.deviceUri;
  }

  // Unpublish nexus.
  async unpublish() {
    log.debug(`Unpublishing nexus "${this}" ...`);

    try {
      await this.node.call('unpublishNexus', { uuid: this.uuid });
    } catch (err) {
      throw new GrpcError(
        GrpcCode.INTERNAL,
        `Failed to unpublish nexus "${this}": ${err}`
      );
    }
    log.info(`Nexus "${this}" was unpublished`);
    this.deviceUri = '';
    this._emitMod();
  }

  // Add replica to the nexus.
  //
  // @param {object} replica   Replica object to add to the nexus.
  //
  async addReplica(replica: Replica) {
    const uri = replica.uri;
    if (this.children.find((ch) => ch.uri === uri)) {
      return;
    }
    log.debug(`Adding uri "${uri}" to nexus "${this}" ...`);

    var childInfo;
    try {
      // TODO: validate the output
      childInfo = await this.node.call('addChildNexus', {
        uuid: this.uuid,
        uri: uri,
        norebuild: false
      });
    } catch (err) {
      throw new GrpcError(
        GrpcCode.INTERNAL,
        `Failed to add uri "${uri}" to nexus "${this}": ${err}`
      );
    }
    // The child will need to be rebuilt when added, but until we get
    // confirmation back from the nexus, set it as pending
    this.children.push(childInfo);
    this.children.sort(compareChildren);
    log.info(`Replica uri "${uri}" added to the nexus "${this}"`);
    this._emitMod();
  }

  // Remove replica from nexus.
  //
  // @param {string} uri   URI of the replica to be removed from the nexus.
  //
  async removeReplica(uri: string) {
    if (!this.children.find((ch) => ch.uri === uri)) {
      return;
    }

    log.debug(`Removing uri "${uri}" from nexus "${this}" ...`);

    try {
      await this.node.call('removeChildNexus', {
        uuid: this.uuid,
        uri: uri
      });
    } catch (err) {
      throw new GrpcError(
        GrpcCode.INTERNAL,
        `Failed to remove uri "${uri}" from nexus "${this}": ${err}`
      );
    }
    // get index again in case the list changed in the meantime
    const idx = this.children.findIndex((ch) => ch.uri === uri);
    if (idx >= 0) {
      this.children.splice(idx, 1);
    }
    log.info(`Replica uri "${uri}" removed from the nexus "${this}"`);
    this._emitMod();
  }

  // Destroy nexus on storage node.
  async destroy() {
    log.debug(`Destroying nexus "${this}" ...`);

    try {
      await this.node.call('destroyNexus', { uuid: this.uuid });
      log.info(`Destroyed nexus "${this}"`);
    } catch (err) {
      // TODO: make destroyNexus idempotent
      if (err.code !== GrpcCode.NOT_FOUND) {
        throw err;
      }
      log.warn(`Destroyed nexus "${this}" does not exist`);
    }

    this.unbind();
  }
}