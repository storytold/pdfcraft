//! CollabCraft transport for the browser build.
//!
//! The same jobs and events as the desktop worker ([`pdfcraft::cloud`]), but
//! the client is async and the session rides the same-origin cookie the service
//! set at sign-in. There is no device flow and no token file.
//!
//! ## Login and logout routing
//!
//! When the account check returns Unauthenticated the transport reports the
//! fault and the title bar shows Sign in. The login popup opens only when the
//! user clicks Sign in (see [`pdfcraft_ui_egui::web_auth`] and [`pdfcraft_ui_egui::cloud::pump`]).
//!
//! A polling closure watches the popup and, when it closes, stores the key
//! `"pdfcraft:auth_done"` in egui's ephemeral memory and requests a repaint.
//! The update loop sees the flag, clears it, and calls `sign_in()` to force a
//! fresh `Job::Account`.
//!
//! Sign-out navigates to the service root `/` only after `/auth/logout`
//! succeeds. Reloading the PDFCraft page on a failed logout would leave the
//! session cookie active while the user believes they signed out.

use craft_cloud_client::{Client, Error};
use pdfcraft_ui_egui::cloud::{Account, CloudBackend, Event, Fault, Job, Post};
use wasm_bindgen::JsCast as _;
use wasm_bindgen_futures::spawn_local;

/// Hands cloud jobs to async tasks. Cheap to clone.
#[derive(Clone)]
pub struct Service {
    client: Client,
    ctx: egui::Context,
}

impl Service {
    /// Always available when the app is served by CollabCraft on the same
    /// origin. The page names the service; nothing is compiled in.
    pub fn new(ctx: egui::Context) -> Service {
        log::info!("cloud storage through {}", Client::from_page().base());
        Service { client: Client::from_page(), ctx }
    }
}

impl CloudBackend for Service {
    fn submit(&self, job: Job, post: Post) {
        let worker = self.clone();
        spawn_local(async move {
            worker.run(job, post).await;
        });
    }
}

impl Service {
    async fn run(self, job: Job, post: Post) {
        let post = |event: Event| {
            post.send(event);
            self.ctx.request_repaint();
        };
        match job {
            Job::Account => {
                let answer =
                    self.me().await.map(|me| Account { display: me.display, capabilities: me.capabilities, provider_linked: me.provider_linked });
                post(Event::Account(answer));
            }
            Job::LinkStart => match self.client.link_start().await.map_err(fault) {
                Ok(start) => post(Event::Browser { url: start.login_url, handle: Some(start.handle) }),
                Err(f) => post(Event::Linked(Err(f))),
            },
            Job::LinkPoll(handle) => {
                let answer = self.client.link_poll(&handle).await.map_err(fault);
                post(Event::Linked(answer));
            }
            Job::SignOut => {
                if let Err(e) = fetch_logout().await {
                    log::warn!("sign-out failed: {e}");
                    post(Event::Account(Err(Fault::Refused(format!("Sign-out failed: {e}")))));
                    return;
                }
                if let Some(window) = web_sys::window() {
                    let _ = window.location().assign("/");
                }
            }
            Job::List(path) => {
                let entries = self.client.list(&path).await.map_err(fault);
                post(Event::Listed { path, entries });
            }
            Job::SharedWithMe => {
                let answer = self.client.shared_with_me().await.map_err(fault);
                post(Event::Shared(answer));
            }
            Job::Read { entry, into } => {
                let id = entry.id.clone();
                let bytes = self.client.read(&id).await.map_err(fault);
                post(Event::Read { entry, into, bytes });
            }
            Job::Write { uid, entry, bytes } => {
                let id = entry.id.clone();
                let etag = entry.etag.clone();
                let answer = self.client.write(&id, &bytes, etag.as_deref()).await.map_err(fault);
                post(Event::Saved { uid, entry: answer });
            }
            Job::Create { uid, folder, name, bytes } => {
                let answer = self.client.create(&folder, &name, &bytes).await.map_err(fault);
                post(Event::Saved { uid, entry: answer });
            }
            Job::Conversation(id) => {
                let answer = self.client.conversation(&id).await.map_err(fault);
                post(Event::Conversation(answer));
            }
        }
    }

    async fn me(&self) -> Result<craft_cloud_client::Me, Fault> {
        self.client.me().await.map_err(fault)
    }
}

fn fault(e: Error) -> Fault {
    match e {
        Error::Unauthenticated => Fault::Unauthenticated,
        Error::NotLinked => Fault::NotLinked,
        Error::Unreachable(detail) => Fault::Unreachable(detail),
        Error::Refused { code, .. } if code == "conflict" || code == "409" || code == "412" => Fault::Conflict,
        Error::Refused { code, detail } => Fault::Refused(if detail.trim().is_empty() { code } else { detail }),
        Error::Malformed(detail) => Fault::Refused(format!("the cloud service gave an answer PdfCraft could not read: {detail}")),
        Error::Unsendable(detail) => {
            log::error!("cloud request could not be encoded: {detail}");
            Fault::Refused("PdfCraft could not build that request".to_owned())
        }
    }
}

async fn fetch_logout() -> Result<(), String> {
    let global = js_sys::global();
    let fetch = js_sys::Reflect::get(&global, &wasm_bindgen::JsValue::from_str("fetch"))
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
        .ok_or_else(|| "no fetch".to_owned())?;
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    init.set_credentials(web_sys::RequestCredentials::Include);
    let req = web_sys::Request::new_with_str_and_init("/auth/logout", &init).map_err(|e| format!("{e:?}"))?;
    let promised = fetch.call1(global.as_ref(), req.as_ref()).map_err(|e| format!("{e:?}"))?;
    let promised: js_sys::Promise = promised.dyn_into().map_err(|_| "not a promise".to_owned())?;
    let resp = wasm_bindgen_futures::JsFuture::from(promised).await.map_err(|e| format!("{e:?}"))?;
    let resp: web_sys::Response = resp.dyn_into().map_err(|_| "not a response".to_owned())?;
    if resp.ok() {
        Ok(())
    } else {
        Err(format!("HTTP {}", resp.status()))
    }
}
