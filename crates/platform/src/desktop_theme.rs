//! Linux desktop appearance through the XDG Settings portal. Other platforms use native window
//! events. No desktop polling or UI dependency is needed here.
//!
//! Protocol reference: <https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Settings.html>.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// A concrete appearance requested by the desktop. An absent value means no preference or no
/// available appearance source; the UI owns its fallback and its saved manual choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DesktopAppearance {
    Light,
    Dark,
}

struct Shared {
    appearance: AtomicU8,
    #[cfg(any(target_os = "linux", test))]
    published: AtomicBool,
    active: AtomicBool,
    #[cfg(any(target_os = "linux", test))]
    wake: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        #[cfg(not(any(target_os = "linux", test)))]
        let _ = wake;
        Self {
            appearance: AtomicU8::new(0),
            #[cfg(any(target_os = "linux", test))]
            published: AtomicBool::new(false),
            active: AtomicBool::new(true),
            #[cfg(any(target_os = "linux", test))]
            wake: Box::new(wake),
        }
    }

    fn get(&self) -> Option<DesktopAppearance> {
        match self.appearance.load(Ordering::Acquire) {
            1 => Some(DesktopAppearance::Dark),
            2 => Some(DesktopAppearance::Light),
            _ => None,
        }
    }

    #[cfg(any(target_os = "linux", test))]
    fn publish(&self, appearance: Option<DesktopAppearance>) {
        let encoded = match appearance {
            Some(DesktopAppearance::Dark) => 1,
            Some(DesktopAppearance::Light) => 2,
            None => 0,
        };
        let changed = self.appearance.swap(encoded, Ordering::AcqRel) != encoded;
        let first = !self.published.swap(true, Ordering::AcqRel);
        // Publish before waking: an otherwise idle UI must see the new answer on its next frame.
        if (first || changed) && self.active.load(Ordering::Acquire) {
            (self.wake)();
        }
    }
}

/// An event-driven appearance cache. Start when System is first selected and keep it alive while
/// the window exists. Manual modes can suppress wakeups while preserving the latest desktop value.
pub struct DesktopTheme {
    shared: Arc<Shared>,
    #[cfg(any(target_os = "linux", test))]
    stop: Option<async_channel::Sender<()>>,
    #[cfg(any(target_os = "linux", test))]
    worker: Option<std::thread::JoinHandle<()>>,
}

impl DesktopTheme {
    /// Start a Linux portal subscription off the UI thread. The first answer and subsequent
    /// changes call `wake` while active. Missing services and thread-start failure yield `None`.
    /// On other platforms this is inert because the window integration owns appearance events.
    pub fn start(wake: impl Fn() + Send + Sync + 'static) -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::spawn(wake, portal::watch)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self {
                shared: Arc::new(Shared::new(wake)),
                #[cfg(test)]
                stop: None,
                #[cfg(test)]
                worker: None,
            }
        }
    }

    /// Get the latest desktop answer without blocking or making a D-Bus call.
    pub fn get(&self) -> Option<DesktopAppearance> {
        self.shared.get()
    }

    /// Allow UI wakeups only while following the desktop. Events still update the cache in
    /// manual mode, so selecting System again can immediately resolve the current appearance.
    pub fn set_active(&self, active: bool) {
        self.shared.active.store(active, Ordering::Release);
    }

    #[cfg(any(target_os = "linux", test))]
    fn spawn<F, T>(wake: impl Fn() + Send + Sync + 'static, task: T) -> Self
    where
        F: std::future::Future<Output = ()> + 'static,
        T: FnOnce(Arc<Shared>) -> F + Send + 'static,
    {
        let shared = Arc::new(Shared::new(wake));
        let thread_shared = Arc::clone(&shared);
        let (stop, stopped) = async_channel::bounded::<()>(1);
        let worker = std::thread::Builder::new().name("pdfcraft-desktop-theme".into()).spawn(move || {
            futures_lite::future::block_on(futures_lite::future::race(
                async {
                    let _ = stopped.recv().await;
                },
                task(thread_shared),
            ));
        });
        Self { shared, stop: Some(stop), worker: worker.ok() }
    }
}

impl Drop for DesktopTheme {
    fn drop(&mut self) {
        self.set_active(false);
        #[cfg(any(target_os = "linux", test))]
        {
            if let Some(stop) = self.stop.take() {
                stop.close();
            }
            if let Some(worker) = self.worker.take() {
                // Cancellation also interrupts connection setup, the initial read, and idle waits.
                // A worker panic does not propagate into the window or its open document.
                let _ = worker.join();
            }
        }
    }
}

#[cfg(any(target_os = "linux", test))]
mod portal {
    use super::{DesktopAppearance, Shared};
    use futures_lite::StreamExt;
    use std::sync::Arc;
    use zbus::zvariant::{OwnedValue, Value};

    const DESTINATION: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const INTERFACE: &str = "org.freedesktop.portal.Settings";
    const NAMESPACE: &str = "org.freedesktop.appearance";
    const KEY: &str = "color-scheme";
    /// The colour scheme under its GNOME schema name, as a string. GNOME's portal answers it, and
    /// so does XFCE's, which proxies the GSettings schemas but not the freedesktop namespace. KDE
    /// answers only the freedesktop one, so that one wins when it states a preference.
    const GNOME_NAMESPACE: &str = "org.gnome.desktop.interface";

    /// Standard uint32 semantics, including future values treated as no preference.
    fn numeric(value: &Value<'_>) -> Option<Option<DesktopAppearance>> {
        match value {
            Value::U32(1) => Some(Some(DesktopAppearance::Dark)),
            Value::U32(2) => Some(Some(DesktopAppearance::Light)),
            Value::U32(_) => Some(None),
            _ => None,
        }
    }

    /// GNOME's string values; anything but the two preferences (`default`) is no preference.
    fn gnome_scheme(value: &Value<'_>) -> Option<Option<DesktopAppearance>> {
        match value {
            Value::Str(scheme) => Some(match scheme.as_str() {
                "prefer-dark" => Some(DesktopAppearance::Dark),
                "prefer-light" => Some(DesktopAppearance::Light),
                _ => None,
            }),
            _ => None,
        }
    }

    /// The freedesktop answer when it states a preference, otherwise the GNOME schema's.
    fn combined(freedesktop: Option<DesktopAppearance>, gnome: Option<DesktopAppearance>) -> Option<DesktopAppearance> {
        freedesktop.or(gnome)
    }

    fn read_gnome_reply(reply: &zbus::Message) -> Option<DesktopAppearance> {
        let answer = reply.body().deserialize::<OwnedValue>().ok()?;
        // Same two variant layers as the freedesktop Read.
        let Value::Value(inner) = &*answer else { return None };
        gnome_scheme(inner).flatten()
    }

    fn gnome_setting_changed(message: &zbus::Message) -> Option<Option<DesktopAppearance>> {
        let body = message.body();
        let (namespace, key, value) = body.deserialize::<(&str, &str, OwnedValue)>().ok()?;
        if namespace != GNOME_NAMESPACE || key != KEY {
            return None;
        }
        gnome_scheme(&value)
    }

    fn read_reply(reply: &zbus::Message) -> Option<DesktopAppearance> {
        let answer = reply.body().deserialize::<OwnedValue>().ok()?;
        // Legacy Read is available in Settings version 1 and returns two variant layers. The
        // OwnedValue decoder removes the outer layer; unwrap exactly the documented inner one.
        let Value::Value(inner) = &*answer else { return None };
        numeric(inner).flatten()
    }

    fn setting_changed(message: &zbus::Message) -> Option<Option<DesktopAppearance>> {
        let body = message.body();
        let (namespace, key, value) = body.deserialize::<(&str, &str, OwnedValue)>().ok()?;
        if namespace != NAMESPACE || key != KEY {
            return None;
        }
        // SettingChanged has one variant layer, unlike legacy Read. Malformed signals do not
        // discard a valid cached appearance; numeric no-preference signals do clear it.
        numeric(&value)
    }

    pub(super) async fn watch(shared: Arc<Shared>) {
        if watch_inner(&shared).await.is_err() {
            shared.publish(None);
        }
    }

    async fn watch_inner(shared: &Shared) -> zbus::Result<()> {
        let connection = zbus::connection::Builder::session()?.method_timeout(std::time::Duration::from_secs(5)).build().await?;
        watch_connection(shared, connection).await
    }

    fn setting_rule(namespace: &'static str) -> zbus::Result<zbus::MatchRule<'static>> {
        Ok(zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(DESTINATION)?
            .path(PATH)?
            .interface(INTERFACE)?
            .member("SettingChanged")?
            .arg(0, namespace)?
            .arg(1, KEY)?
            .build())
    }

    async fn watch_connection(shared: &Shared, connection: zbus::Connection) -> zbus::Result<()> {
        // Install the matches before reading. Unlike Proxy::receive_signal this does not require
        // the portal to already own its bus name, so Read can activate an installed portal.
        let freedesktop_changes = zbus::MessageStream::for_match_rule(setting_rule(NAMESPACE)?, &connection, Some(64)).await?;
        let gnome_changes = zbus::MessageStream::for_match_rule(setting_rule(GNOME_NAMESPACE)?, &connection, Some(64)).await?;
        let mut changes = freedesktop_changes.or(gnome_changes);
        let reply = connection.call_method(Some(DESTINATION), PATH, Some(INTERFACE), "Read", &(NAMESPACE, KEY)).await;
        let mut freedesktop = reply.as_ref().ok().and_then(read_reply);
        let reply = connection.call_method(Some(DESTINATION), PATH, Some(INTERFACE), "Read", &(GNOME_NAMESPACE, KEY)).await;
        let mut gnome = reply.as_ref().ok().and_then(read_gnome_reply);
        shared.publish(combined(freedesktop, gnome));
        while let Some(message) = changes.next().await {
            let message = message?;
            // Keep every subscribed update, including those queued during Read. A Read handler
            // can capture an old value and emit a new one before returning its older snapshot.
            if let Some(appearance) = setting_changed(&message) {
                freedesktop = appearance;
            } else if let Some(appearance) = gnome_setting_changed(&message) {
                gnome = appearance;
            } else {
                continue;
            }
            shared.publish(combined(freedesktop, gnome));
        }
        shared.publish(None);
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn reply(value: Value<'_>) -> zbus::Message {
            let call = zbus::Message::method_call(PATH, "Read").unwrap().build(&(NAMESPACE, KEY)).unwrap();
            zbus::Message::method_return(&call.header()).unwrap().build(&value).unwrap()
        }

        fn signal(namespace: &str, key: &str, value: Value<'_>) -> zbus::Message {
            zbus::Message::signal(PATH, INTERFACE, "SettingChanged").unwrap().build(&(namespace, key, value)).unwrap()
        }

        #[test]
        fn legacy_read_decodes_documented_double_variant_numeric_values() {
            // Keep the native subscription implementation typechecked on non-Linux test hosts.
            let _native_backend = watch;
            for (number, expected) in [(0, None), (1, Some(DesktopAppearance::Dark)), (2, Some(DesktopAppearance::Light)), (u32::MAX, None)] {
                assert_eq!(read_reply(&reply(Value::Value(Box::new(Value::U32(number))))), expected);
            }
        }

        #[test]
        fn legacy_read_refuses_strings_wrong_types_and_extra_nesting() {
            assert_eq!(read_reply(&reply(Value::Value(Box::new(Value::from("prefer-dark"))))), None);
            assert_eq!(read_reply(&reply(Value::Value(Box::new(Value::I32(1))))), None);
            assert_eq!(read_reply(&reply(Value::Value(Box::new(Value::Value(Box::new(Value::U32(1))))))), None);
            assert_eq!(read_reply(&reply(Value::U32(1))), None);
        }

        #[test]
        fn signals_distinguish_no_preference_from_unrelated_or_malformed_updates() {
            assert_eq!(setting_changed(&signal(NAMESPACE, KEY, Value::U32(1))), Some(Some(DesktopAppearance::Dark)));
            assert_eq!(setting_changed(&signal(NAMESPACE, KEY, Value::U32(2))), Some(Some(DesktopAppearance::Light)));
            assert_eq!(setting_changed(&signal(NAMESPACE, KEY, Value::U32(0))), Some(None));
            assert_eq!(setting_changed(&signal(NAMESPACE, KEY, Value::U32(1234))), Some(None));
            assert_eq!(setting_changed(&signal("org.gnome.desktop.interface", KEY, Value::U32(1))), None);
            assert_eq!(setting_changed(&signal(NAMESPACE, "contrast", Value::U32(1))), None);
            assert_eq!(setting_changed(&signal(NAMESPACE, KEY, Value::from("prefer-dark"))), None);
            assert_eq!(setting_changed(&signal(NAMESPACE, KEY, Value::Value(Box::new(Value::U32(1))))), None);
            let wrong_body = zbus::Message::signal(PATH, INTERFACE, "SettingChanged").unwrap().build(&(NAMESPACE, KEY)).unwrap();
            assert_eq!(setting_changed(&wrong_body), None);
        }

        #[test]
        fn the_gnome_schema_answers_when_the_freedesktop_one_states_no_preference() {
            // XFCE's portal proxies only the GSettings schemas; its answer is a string.
            assert_eq!(read_gnome_reply(&reply(Value::Value(Box::new(Value::from("prefer-dark"))))), Some(DesktopAppearance::Dark));
            assert_eq!(read_gnome_reply(&reply(Value::Value(Box::new(Value::from("prefer-light"))))), Some(DesktopAppearance::Light));
            assert_eq!(read_gnome_reply(&reply(Value::Value(Box::new(Value::from("default"))))), None);
            assert_eq!(read_gnome_reply(&reply(Value::Value(Box::new(Value::U32(1))))), None);
            assert_eq!(read_gnome_reply(&reply(Value::from("prefer-dark"))), None);
            assert_eq!(gnome_setting_changed(&signal(GNOME_NAMESPACE, KEY, Value::from("prefer-dark"))), Some(Some(DesktopAppearance::Dark)));
            assert_eq!(gnome_setting_changed(&signal(GNOME_NAMESPACE, KEY, Value::from("default"))), Some(None));
            assert_eq!(gnome_setting_changed(&signal(GNOME_NAMESPACE, "gtk-theme", Value::from("prefer-dark"))), None);
            assert_eq!(gnome_setting_changed(&signal(NAMESPACE, KEY, Value::from("prefer-dark"))), None);
            assert_eq!(gnome_setting_changed(&signal(GNOME_NAMESPACE, KEY, Value::U32(1))), None);
            // The freedesktop answer wins when it states one (KDE answers only that one).
            assert_eq!(combined(Some(DesktopAppearance::Light), Some(DesktopAppearance::Dark)), Some(DesktopAppearance::Light));
            assert_eq!(combined(None, Some(DesktopAppearance::Dark)), Some(DesktopAppearance::Dark));
            assert_eq!(combined(None, None), None);
        }

        #[test]
        #[cfg(target_os = "linux")]
        #[ignore = "requires dbus-daemon; uses only a private test session bus"]
        fn private_bus_delayed_read_keeps_queued_changes_and_wakes_idle_tracker() {
            use crate::desktop_theme::DesktopTheme;
            use std::io::BufRead;
            use std::process::{Command, Stdio};
            use std::sync::{OnceLock, Weak, mpsc};
            use std::time::{Duration, Instant};

            struct Daemon(std::process::Child);
            impl Drop for Daemon {
                fn drop(&mut self) {
                    let _ = self.0.kill();
                    let _ = self.0.wait();
                }
            }
            struct Settings {
                captured: mpsc::Sender<()>,
                release: async_channel::Receiver<()>,
            }
            #[zbus::interface(name = "org.freedesktop.portal.Settings")]
            impl Settings {
                async fn read(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
                    if namespace != NAMESPACE || key != KEY {
                        return Err(zbus::fdo::Error::InvalidArgs("unknown setting".into()));
                    }
                    // Capture Light before a later Dark signal, then deliberately return the
                    // stale snapshot after that signal. Reply ordering cannot date this value.
                    let snapshot = Value::Value(Box::new(Value::U32(2)));
                    self.captured.send(()).map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
                    self.release.recv().await.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
                    OwnedValue::try_from(snapshot).map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
                }
            }
            fn wait_for(tracker: &DesktopTheme, appearance: Option<DesktopAppearance>) {
                let deadline = Instant::now() + Duration::from_secs(5);
                while tracker.get() != appearance {
                    assert!(Instant::now() < deadline, "tracker did not consume the setting signal");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            fn emit(connection: &zbus::Connection, number: u32) {
                futures_lite::future::block_on(connection.emit_signal(
                    None::<&str>,
                    PATH,
                    INTERFACE,
                    "SettingChanged",
                    &(NAMESPACE, KEY, OwnedValue::from(number)),
                ))
                .unwrap();
            }

            let child = Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                // Abstract sockets disappear with the daemon, even if a test fails.
                .arg(format!("--address=unix:abstract=pdfcraft-theme-test-{}", std::process::id()))
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let mut daemon = Daemon(child);
            let mut address = String::new();
            std::io::BufReader::new(daemon.0.stdout.take().unwrap()).read_line(&mut address).unwrap();
            assert!(!address.trim().is_empty());
            let (captured, capture_seen) = mpsc::channel();
            let (release, released) = async_channel::bounded(1);
            let server = futures_lite::future::block_on(
                zbus::connection::Builder::address(address.trim())
                    .unwrap()
                    .name(DESTINATION)
                    .unwrap()
                    .serve_at(PATH, Settings { captured, release: released })
                    .unwrap()
                    .build(),
            )
            .unwrap();
            let client = futures_lite::future::block_on(
                zbus::connection::Builder::address(address.trim()).unwrap().method_timeout(Duration::from_secs(5)).build(),
            )
            .unwrap();
            let source = Arc::new(OnceLock::<Weak<Shared>>::new());
            let observed = Arc::clone(&source);
            let (wake, woke) = mpsc::channel();
            let tracker = DesktopTheme::spawn(
                move || {
                    wake.send(observed.get().unwrap().upgrade().unwrap().get()).unwrap();
                },
                move |shared| async move {
                    watch_connection(&shared, client).await.unwrap();
                },
            );
            source.set(Arc::downgrade(&tracker.shared)).unwrap();
            capture_seen.recv_timeout(Duration::from_secs(5)).unwrap();
            emit(&server, 1);
            release.send_blocking(()).unwrap();
            assert_eq!(woke.recv_timeout(Duration::from_secs(5)).unwrap(), Some(DesktopAppearance::Light));
            assert_eq!(woke.recv_timeout(Duration::from_secs(5)).unwrap(), Some(DesktopAppearance::Dark));
            assert_eq!(tracker.get(), Some(DesktopAppearance::Dark));

            emit(&server, 2);
            assert_eq!(woke.recv_timeout(Duration::from_secs(5)).unwrap(), Some(DesktopAppearance::Light));
            emit(&server, 0);
            assert_eq!(woke.recv_timeout(Duration::from_secs(5)).unwrap(), None);
            tracker.set_active(false);
            emit(&server, 1);
            wait_for(&tracker, Some(DesktopAppearance::Dark));
            emit(&server, 2);
            wait_for(&tracker, Some(DesktopAppearance::Light));
            assert!(matches!(woke.try_recv(), Err(mpsc::TryRecvError::Empty)));
            tracker.set_active(true);
            assert_eq!(tracker.get(), Some(DesktopAppearance::Light));
            emit(&server, 1);
            assert_eq!(woke.recv_timeout(Duration::from_secs(5)).unwrap(), Some(DesktopAppearance::Dark));
            // Closing the tracker must cancel an idle bus subscription and join the worker.
            drop(tracker);
            drop(server);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn initial_and_changed_results_wake_after_publication_but_duplicates_do_not() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&wakes);
        let source = Arc::new(std::sync::OnceLock::<std::sync::Weak<Shared>>::new());
        let observed_source = Arc::clone(&source);
        let (snapshot, snapshots) = std::sync::mpsc::channel();
        let shared = Arc::new(Shared::new(move || {
            snapshot.send(observed_source.get().unwrap().upgrade().unwrap().get()).unwrap();
            observed.fetch_add(1, Ordering::Relaxed);
        }));
        source.set(Arc::downgrade(&shared)).unwrap();
        shared.publish(None);
        assert_eq!(snapshots.try_recv().unwrap(), None);
        assert_eq!(wakes.load(Ordering::Relaxed), 1);
        shared.publish(None);
        assert_eq!(wakes.load(Ordering::Relaxed), 1);
        shared.publish(Some(DesktopAppearance::Dark));
        assert_eq!(snapshots.try_recv().unwrap(), Some(DesktopAppearance::Dark));
        assert_eq!(shared.get(), Some(DesktopAppearance::Dark));
        assert_eq!(wakes.load(Ordering::Relaxed), 2);
        shared.publish(Some(DesktopAppearance::Dark));
        assert_eq!(wakes.load(Ordering::Relaxed), 2);
        shared.publish(Some(DesktopAppearance::Light));
        assert_eq!(snapshots.try_recv().unwrap(), Some(DesktopAppearance::Light));
        assert_eq!(shared.get(), Some(DesktopAppearance::Light));
        assert_eq!(wakes.load(Ordering::Relaxed), 3);
        shared.publish(None);
        assert_eq!(snapshots.try_recv().unwrap(), None);
        assert_eq!(shared.get(), None);
        assert_eq!(wakes.load(Ordering::Relaxed), 4);
        assert!(matches!(snapshots.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
    }

    #[test]
    fn manual_mode_caches_changes_without_waking_and_system_resumes_latest_value() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&wakes);
        let tracker = DesktopTheme::spawn(
            move || {
                observed.fetch_add(1, Ordering::Relaxed);
            },
            |_| futures_lite::future::pending(),
        );
        tracker.set_active(false);
        tracker.shared.publish(Some(DesktopAppearance::Dark));
        tracker.shared.publish(Some(DesktopAppearance::Light));
        assert_eq!(tracker.get(), Some(DesktopAppearance::Light));
        assert_eq!(wakes.load(Ordering::Relaxed), 0);
        tracker.set_active(true);
        assert_eq!(tracker.get(), Some(DesktopAppearance::Light));
        tracker.shared.publish(Some(DesktopAppearance::Dark));
        assert_eq!(wakes.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn dropping_tracker_cancels_and_joins_an_idle_worker() {
        struct Finished(Arc<AtomicBool>);
        impl Drop for Finished {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let (ready, waiting) = std::sync::mpsc::channel();
        let tracker = DesktopTheme::spawn(
            || {},
            move |_| async move {
                let _finished = Finished(worker_finished);
                ready.send(()).unwrap();
                futures_lite::future::pending::<()>().await;
            },
        );
        waiting.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        drop(tracker);
        assert!(finished.load(Ordering::Acquire));
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn native_non_linux_tracker_is_inert() {
        let tracker = DesktopTheme::start(|| panic!("non-Linux tracker must not wake"));
        assert_eq!(tracker.get(), None);
        tracker.set_active(false);
        tracker.set_active(true);
    }
}
