#![feature(generators, generator_trait)]

#[macro_use]
extern crate neon;
extern crate carrier;
extern crate log;

use neon::prelude::*;
use carrier::osaka::{self, osaka};
use std::sync::{Arc, Mutex};
use std::io::Write;
use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::thread;
use std::time::Duration;

fn identity(mut cx: FunctionContext) -> JsResult<JsString> {
    let config = carrier::config::load().unwrap();
    Ok(cx.string(config.secret.identity().to_string()))
}




register_module!(mut cx, {
    cx.export_function("identity",  identity)?;
    cx.export_function("get",       simple_get)?;
    cx.export_class::<JsConduit>("ConduitW")?;
    cx.export_class::<JsPeer>("Peer")?;
    Ok(())
});


// conduit


pub struct Peer {
    identity:   Option<carrier::Identity>,
    broker:     Option<Arc<Mutex<carrier::conduit::ConduitState>>>,
    setup:      Option<carrier::conduit::PeerSetup>,
}
declare_types! {
    pub class JsPeer for Peer {
        init(_) {
            Ok(Peer{
                broker:     None,
                identity:   None,
                setup:      None,
            })
        }

        method connect(mut cx) {
            {
                let mut this = cx.this();
                let guard = cx.lock();
                let mut that = this.borrow_mut(&guard);
                let identity = that.identity.clone().unwrap();
                let setup = that.broker.as_mut().unwrap().lock().unwrap().connect(identity);
                that.setup = Some(setup);
            };

            Ok(cx.undefined().upcast())
        }


        method discover(mut cx) {
            let callback = cx.argument::<JsFunction>(0)?;
            let callback  = EventHandler::new(cx.this(), callback);
            {
                let mut this = cx.this();
                let guard = cx.lock();
                let mut that = this.borrow_mut(&guard);
                that.setup.as_mut().expect("not connected").discovery(
                    move |disco|{
                        callback.schedule(move |cx, this, callback| {
                            let object = JsObject::new(cx);

                            let v = cx.number(disco.carrier_revision as f64);
                            object.set(cx, "carrier_revision", v).unwrap();

                            let v = cx.string(disco.carrier_build_id);
                            object.set(cx, "carrier_build_id", v).unwrap();

                            let v = cx.string(disco.application);
                            object.set(cx, "application", v).unwrap();

                            let v = cx.string(disco.application_version);
                            object.set(cx, "application_version", v).unwrap();


                            let v = JsArray::new(cx, disco.paths.len() as u32);
                            for (i, s) in disco.paths.iter().enumerate() {
                                let js_string = cx.string(s);
                                v.set(cx, i as u32, js_string).unwrap();
                            }
                            object.set(cx, "paths", v).unwrap();



                            let args    : Vec<Handle<JsValue>> = vec![
                                object.upcast(),
                            ];
                            callback.call(cx, this, args).unwrap();
                        });
                    }
                );
            };
            Ok(cx.undefined().upcast())
        }

        method schedule(mut cx) {
            let seconds = cx.argument::<JsNumber>(0)?.value();
            let mut headers = carrier::headers::Headers::new();
            let jsheaders   = cx.argument::<JsObject>(1)?;
            let keys = jsheaders.get_own_property_names(&mut cx)?.to_vec(&mut cx)?;
            for key in keys {
                if let Ok(key) = key.to_string(&mut cx) {
                    if let Ok(value) = jsheaders.get(&mut cx, key) {
                        if let Ok(value) = value.to_string(&mut cx) {
                            headers.add(key.value().into(), value.value().into());
                        }
                    }
                }
            }
            let callback = cx.argument::<JsFunction>(2)?;
            let callback  = EventHandler::new(cx.this(), callback);

            {
                let mut this = cx.this();
                let guard = cx.lock();
                let mut that = this.borrow_mut(&guard);
                that.setup.as_mut().expect("not connected").schedule_raw(
                    std::time::Duration::from_secs(seconds as u64),
                    headers,
                    move |identity, bytes|{
                        let identity = identity.to_string();
                        callback.schedule(move |cx, this, callback| {
                            let mut b = JsArrayBuffer::new(cx, bytes.len() as u32).unwrap();
                            cx.borrow_mut(&mut b, |data| {
                                let mut slice = data.as_mut_slice::<u8>();
                                slice.write_all(&bytes).unwrap();
                            });
                            let args    : Vec<Handle<JsValue>> = vec![
                                cx.string(identity).upcast(),
                                b.upcast(),
                            ];
                            callback.call(cx, this, args).unwrap();
                        });
                    }
                );
            };
            Ok(cx.undefined().upcast())
        }
    }
}

pub struct Conduit {
    emit: Option<EventHandler>,
    on_publish: Option<EventHandler>,
}

declare_types! {
    pub class JsConduit for Conduit{
        init(_) {
            Ok(Conduit{
                emit:       None,
                on_publish: None,
            })
        }

        constructor(mut cx) {
            if let Err(_) = std::env::var("RUST_LOG") {
                std::env::set_var("RUST_LOG", "info");
            }
            env_logger::try_init().ok();


            let mut this = cx.this();
            let f = this.get(&mut cx, "emit")?.downcast::<JsFunction>().or_throw(&mut cx)?;
            let emit = EventHandler::new(this, f);
            {
                let guard = cx.lock();
                let mut that = this.borrow_mut(&guard);
                that.emit   = Some(emit);
            }

            let f = cx.argument::<JsFunction>(0)?;
            let on_publish = EventHandler::new(this, f);
            {
                let guard = cx.lock();
                let mut that = this.borrow_mut(&guard);
                that.on_publish = Some(on_publish);
            }

            Ok(None)
        }

        method start(mut cx) {
            let this = cx.this();
            let on_publish = {
                let guard = cx.lock();
                let callback = this.borrow(&guard);
                callback.on_publish.clone()
            };

            thread::spawn(move || {
                let conduit = carrier::conduit::Builder::new(carrier::config::load().unwrap()).unwrap();
                conduit.start(move |identity: carrier::Identity, broker: Arc<Mutex<carrier::conduit::ConduitState>>| {
                    on_publish.as_ref().unwrap().schedule(move |cx, this, callback| {

                        let args    : Vec<Handle<JsValue>> = vec![
                            cx.string(identity.to_string()).upcast(),
                        ];
                        let r = match callback.call(cx, this, args) {
                            Err(e) => {
                                eprintln!("{:?}", e);
                                return;
                            },
                            Ok(v) => v,
                        };
                        let mut peer : Handle<JsPeer> = match r.downcast() {
                            Err(e) => {
                                eprintln!("conduit cb did not return JsPeer {:?}", e);
                                return;
                            },
                            Ok(v) => v,
                        };
                        cx.borrow_mut(&mut peer, |mut peer|{
                            peer.broker     = Some(broker);
                            peer.identity   = Some(identity.clone());
                        });

                        let this : Handle<JsObject> = this.downcast().unwrap();
                        let emit = this.get(cx, "emit").unwrap().downcast::<JsFunction>().or_throw(cx).unwrap();

                        let args    : Vec<Handle<JsValue>> = vec![
                            cx.string("publish".to_string()).upcast(),
                            cx.string(identity.to_string()).upcast(),
                            peer.upcast(),
                        ];
                        let r = match emit.call(cx, this, args) {
                            Err(e) => {
                                eprintln!("{:?}", e);
                                return;
                            },
                            Ok(v) => v,
                        };

                    });
                })
            });

            Ok(cx.undefined().upcast())
        }

        method discover(mut cx) {
            let f = cx.argument::<JsFunction>(0)?;
            Ok(cx.undefined().upcast())
        }

        method shutdown(mut cx) {
            let mut this = cx.this();
            {
                let guard = cx.lock();
                let mut callback = this.borrow_mut(&guard);
                callback.emit = None;
            }
            Ok(cx.undefined().upcast())
        }
    }
}





// get



struct SimpleGetTask{
    identity: carrier::Identity,
    headers:  carrier::headers::Headers,
}

impl Task for SimpleGetTask {
    // If the computation does not error, it will return an i32.
    // Otherwise, it will return a String as an error
    type Output     = (carrier::headers::Headers, Option<Vec<u8>>);
    type Error      = String;
    type JsEvent    = JsObject;

    // Perform expensive computation here. What runs in here
    // will not block the main thread. Will run in a background
    // thread
    fn perform(&self) -> Result<(carrier::headers::Headers, Option<Vec<u8>>), String> {

        let rr = Arc::new(Mutex::new(None));
        let rr2 = rr.clone();
        let rh = Arc::new(Mutex::new(None));
        let rh2 = rh.clone();

        let config = carrier::config::load().unwrap();

        carrier::connect(config).open(
            self.identity.clone(),
            self.headers.clone(),
            move |poll, ep, stream| return_one(poll, ep, stream, rh, rr),
            ).run().unwrap();

        let rr2 = Arc::try_unwrap(rr2).unwrap().into_inner().unwrap();
        let rh2 = match Arc::try_unwrap(rh2).unwrap().into_inner().unwrap() {
            None => return Err("received no headers".into()),
            Some(v) => v,
        };


        Ok((rh2, rr2))
    }

    // When perform() is finished running, complete() will convert
    // the result of the task to a JS value. In this case we are
    // converting a Rust i32 to a JsNumber. This value will be passed
    // to the callback. perform() is executed on the main thread at
    // some point after the background task is completed.
    fn complete(self, mut cx: TaskContext, result: Result<(carrier::headers::Headers, Option<Vec<u8>>), String>) -> JsResult<JsObject> {

        let (rh, rr) =  match result {
            Err(e) => {
                let e = cx.string(e);
                return cx.throw(e);
            },
            Ok(v)  => v,
        };
        let h = cx.empty_object();
        for (k,v) in rh.iter() {
            let v = cx.string(String::from_utf8_lossy(&v));
            h.set(&mut cx, String::from_utf8_lossy(&k).as_ref(), v)?;
        }

        let obj = cx.empty_object();
        obj.set(&mut cx, "headers", h)?;



        match rr {
            Some(v) => {
                let mut b = JsArrayBuffer::new(&mut cx, v.len() as u32)?;
                cx.borrow_mut(&mut b, |data| {
                    let mut slice = data.as_mut_slice::<u8>();
                    slice.write_all(&v).unwrap();
                });
                obj.set(&mut cx, "data", b)?;
            },
            None  => {
            }
        };

        return Ok(obj);
    }
}

pub fn simple_get(mut cx: FunctionContext) -> JsResult<JsUndefined> {

    let identity  : carrier::Identity = (cx.argument::<JsString>(0)?).value().parse().unwrap();

    let mut headers = carrier::headers::Headers::new();
    let jsheaders   = cx.argument::<JsObject>(1)?;
    let keys = jsheaders.get_own_property_names(&mut cx)?.to_vec(&mut cx)?;
    for key in keys {
        if let Ok(key) = key.to_string(&mut cx) {
            if let Ok(value) = jsheaders.get(&mut cx, key) {
                if let Ok(value) = value.to_string(&mut cx) {
                    headers.add(key.value().into(), value.value().into());
                }
            }
        }
    }

    let f        = cx.argument::<JsFunction>(2)?;
    SimpleGetTask{identity, headers}.schedule(f);
    Ok(cx.undefined())
}

#[osaka]
fn return_one(
    _poll:  osaka::Poll,
    ep:     carrier::endpoint::Handle,
    mut     stream: carrier::endpoint::Stream,
    rh:     Arc<Mutex<Option<carrier::headers::Headers>>>,
    rr:     Arc<Mutex<Option<Vec<u8>>>>,
    ) {
    let _d = carrier::util::defer(move || {
        ep.disconnect(ep.broker(), carrier::packet::DisconnectReason::Application);
    });

    let headers = carrier::headers::Headers::decode(&osaka::sync!(stream)).unwrap();

    let status = headers.status().unwrap_or(999);
    *rh.lock().unwrap() = Some(headers);
    if status > 299 {
        return;
    }

    let r = osaka::sync!(stream);
    *rr.lock().unwrap() = Some(r);
}

