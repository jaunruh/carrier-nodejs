var addon   = require('../native');
var events  = require('events');
var util  = require('util');

util.inherits(addon.Peer,    events.EventEmitter);
util.inherits(addon.ConduitW, events.EventEmitter);

var extend_with_alloc = function(cls) {
  function allocator () {
    return new addon.Peer();
  };

  function nutype() {
    let r = new addon.ConduitW(allocator);
    r.prototype = Object.create(cls.prototype);
    return r;
  }

  return nutype;
};

addon.Conduit = extend_with_alloc(addon.ConduitW);


module.exports = addon;
