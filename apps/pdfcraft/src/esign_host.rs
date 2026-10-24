//! Host half of the e-sign dialog: CollabCraft HTTP on a worker thread.

use std::sync::mpsc::Sender;

use craft_cloud_client::Client;
use pdfcraft_ui_egui::esign::{Answer, EsignBackend, Replier, Request};

use crate::cloud;

/// Worker handle implementing [`EsignBackend`].
pub struct Service {
    jobs: Sender<(Request, Replier)>,
}

impl EsignBackend for Service {
    fn submit(&self, request: Request, reply: Replier) {
        if self.jobs.send((request, reply)).is_err() {
            log::warn!("the e-sign worker is gone; the request was dropped");
        }
    }
}

/// Start the worker when a CollabCraft URL is configured.
pub fn service(ctx: egui::Context) -> Option<Service> {
    let _ = cloud::client()?;
    let (jobs, receiver) = std::sync::mpsc::channel::<(Request, Replier)>();
    let started = std::thread::Builder::new().name("esign".to_owned()).spawn(move || {
        while let Ok((request, reply)) = receiver.recv() {
            let Some(client) = cloud::client() else {
                reply.refuse("cloud storage is not configured");
                ctx.request_repaint();
                continue;
            };
            run(&client, request, reply);
            ctx.request_repaint();
        }
    });
    if let Err(e) = started {
        log::error!("the e-sign worker did not start: {e}");
        return None;
    }
    Some(Service { jobs })
}

fn run(client: &Client, request: Request, reply: Replier) {
    match request {
        Request::Capabilities => match client.esign_capabilities() {
            Ok(caps) => {
                reply.answer(Answer::Capabilities { capabilities: caps.capabilities, responsible_party: caps.responsible_party, linked: caps.linked })
            }
            Err(e) => reply.refuse(e.to_string()),
        },
        Request::Lookup { query, limit } => match client.contacts(&query, limit) {
            Ok(contacts) => reply.answer(Answer::Lookup(contacts)),
            Err(e) => reply.refuse(e.to_string()),
        },
        Request::Create { draft, document, idempotency_key, send } => match client.esign_create(draft.as_ref(), &document, &idempotency_key, send) {
            Ok(env) => reply.answer(Answer::Created(env)),
            Err(e) => reply.refuse(e.to_string()),
        },
        Request::Status { envelope } => match client.esign_status(&envelope) {
            Ok(env) => reply.answer(Answer::Status(env)),
            Err(e) => reply.refuse(e.to_string()),
        },
        Request::Completed { envelope } => match client.esign_completed(&envelope) {
            Ok(doc) => reply.answer(Answer::Completed(doc)),
            Err(e) => reply.refuse(e.to_string()),
        },
        Request::Void { envelope, reason } => match client.esign_void(&envelope, &reason) {
            Ok(()) => reply.answer(Answer::Voided { envelope }),
            Err(e) => reply.refuse(e.to_string()),
        },
        Request::Remind { envelope, recipient } => match client.esign_remind(&envelope, &recipient) {
            Ok(()) => reply.answer(Answer::Reminded { envelope, recipient }),
            Err(e) => reply.refuse(e.to_string()),
        },
    }
}
