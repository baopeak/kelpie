//! Where messages go: each channel that is on and each phone with alerts on is a destination with its own bounded queue, thread and
//! outbox, so that a server that hangs holds up nothing but itself.
//!
//! Nothing here exists until a message is sent to a destination, and a destination that goes takes its thread and its file with it:
//! with none there is no thread, no socket and no timer. What decides whether an event leaves at all (`Notifier::wants_push`) runs on
//! the classifier's thread and takes no lock a worker holds across a request; everything that can block is on the worker's thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use parking_lot::Mutex;

use super::outbox::{Attempt, Entry, Outbox, Pick};
use super::web_push::{self, Vapid, Wording};
use super::{channel_on, deep_link, deliver, push_test_base, record, telegram_base, Channel, ChannelStatus, Config, Failure, Kind, Payload, SecretStore, QUEUE};
use crate::load::Level;
use crate::session::now_millis;

/// The daemon's signing key for Web Push.
pub const K_VAPID: &str = "mobile-push-vapid";
/// Name in the credential store of a phone's subscription secret.
pub fn auth_key(device: &str) -> String {
    format!("mobile-push-auth-{device}")
}

/// A phone with alerts on. The secret that goes with it is in the credential store.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub endpoint: String,
    pub p256dh: String,
    pub lang: String,
    /// The origin of the page that subscribed, which is where its pairing is. None for a subscription from before it was recorded.
    pub origin: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Dest {
    Channel(Channel),
    Device(String),
}

const CHANNELS: [Channel; 3] = [Channel::Ntfy, Channel::Telegram, Channel::Webhook];
/// A second send of a message that may have arrived waits at least this: what a channel always waited, so that a blip passes.
const SECOND_SEND_AFTER: Duration = Duration::from_secs(3);

/// What delivery asks of the daemon around it.
pub trait Host: Send + Sync {
    /// The session still waits on the person, on this wait.
    fn still_waiting(&self, _session: &str, _wait: Option<u64>) -> bool {
        true
    }
    fn load_level(&self) -> Level {
        Level::L0
    }
    /// The run's task still waits for approval. What is true of a run is the run's to say: it outlives a restart.
    fn task_waiting(&self, _run: &str, _task: &str) -> bool {
        true
    }
    /// The phone page's address, for the link in an alert to a phone whose own origin is not known. None while there is no right one.
    fn phone_url(&self) -> Option<String> {
        None
    }
    /// The push service says this phone's subscription at this address is over.
    fn subscription_gone(&self, _device: &str, _endpoint: Option<&str>) {}
}

#[cfg(test)]
pub struct NoHost;
#[cfg(test)]
impl Host for NoHost {}

struct Shared {
    dir: PathBuf,
    secrets: Arc<dyn SecretStore>,
    host: Arc<dyn Host>,
    config: Arc<Mutex<Config>>,
    status: Arc<Mutex<HashMap<&'static str, ChannelStatus>>>,
    dropped: Arc<AtomicU64>,
    /// `notify.json` said what is on (it may not have been there). A file of a channel that seems off is not deleted on the word of one it could not read.
    config_known: AtomicBool,
    /// The same of who has alerts on, from the device store and the key that checks it.
    devices_known: AtomicBool,
    devices: Mutex<HashMap<String, Target>>,
    vapid: Mutex<Option<Arc<Vapid>>>,
    /// A unit test's stand-in for a push service (the environment is one per process, and tests run side by side).
    #[cfg(test)]
    test_base: Mutex<Option<String>>,
}

/// A destination's thread as seen from here.
struct Worker {
    tx: mpsc::SyncSender<Entry>,
    /// Its destination was taken away. It touches the file no more: a new thread may be using it.
    retired: Arc<AtomicBool>,
}

pub struct Destinations {
    shared: Arc<Shared>,
    workers: Mutex<HashMap<Dest, Worker>>,
    device_count: AtomicUsize,
}

fn file_name(dest: &Dest) -> String {
    match dest {
        Dest::Channel(c) => format!("{}.json", c.code()),
        // A hash: what names the file comes out of a store, and is no path.
        Dest::Device(id) => format!("device-{}.json", web_push::topic(id)),
    }
}

fn path_of(dir: &Path, dest: &Dest) -> PathBuf {
    dir.join(file_name(dest))
}

impl Destinations {
    pub fn new(data_dir: &Path, secrets: Arc<dyn SecretStore>, host: Arc<dyn Host>) -> Arc<Destinations> {
        Arc::new(Destinations {
            shared: Arc::new(Shared {
                dir: data_dir.join("notify-outbox"),
                secrets,
                host,
                config: Arc::new(Mutex::new(Config::default())),
                status: Arc::new(Mutex::new(HashMap::new())),
                dropped: Arc::new(AtomicU64::new(0)),
                config_known: AtomicBool::new(true),
                devices_known: AtomicBool::new(true),
                devices: Mutex::new(HashMap::new()),
                vapid: Mutex::new(None),
                #[cfg(test)]
                test_base: Mutex::new(None),
            }),
            workers: Mutex::new(HashMap::new()),
            device_count: AtomicUsize::new(0),
        })
    }

    pub(super) fn config(&self) -> Arc<Mutex<Config>> {
        self.shared.config.clone()
    }

    pub(super) fn status(&self) -> Arc<Mutex<HashMap<&'static str, ChannelStatus>>> {
        self.shared.status.clone()
    }

    pub(super) fn dropped(&self) -> Arc<AtomicU64> {
        self.shared.dropped.clone()
    }

    pub(super) fn set_config_known(&self, known: bool) {
        self.shared.config_known.store(known, Ordering::Relaxed);
    }

    pub fn set_devices_known(&self, known: bool) {
        self.shared.devices_known.store(known, Ordering::Relaxed);
    }

    /// This phone is among those being sent to.
    pub fn has_device(&self, id: &str) -> bool {
        self.shared.devices.lock().contains_key(id)
    }

    /// What a test run may send to besides the push services' own hosts.
    pub fn test_base(&self) -> Option<String> {
        self.shared.test_base()
    }

    pub fn host(&self) -> &Arc<dyn Host> {
        &self.shared.host
    }

    /// How many phones have alerts on (cheap: read on every status change).
    pub fn device_count(&self) -> usize {
        self.device_count.load(Ordering::Relaxed)
    }

    pub fn device_ids(&self) -> Vec<String> {
        self.shared.devices.lock().keys().cloned().collect()
    }

    /// The phones that have alerts on, as the store says now. One that is no longer among them stops here, before this returns: its
    /// thread ends, its file goes, and what it still had to send goes with it.
    pub fn set_devices(&self, next: HashMap<String, Target>) {
        let gone: Vec<String> = {
            let mut devices = self.shared.devices.lock();
            let gone = devices.keys().filter(|id| !next.contains_key(*id)).cloned().collect();
            self.device_count.store(next.len(), Ordering::Relaxed);
            *devices = next;
            gone
        };
        for id in gone {
            self.remove(&Dest::Device(id));
        }
    }

    /// A channel switched off stops the same way.
    pub fn channels_changed(&self, config: &Config) {
        for ch in CHANNELS {
            if !channel_on(config, ch) {
                self.remove(&Dest::Channel(ch));
            }
        }
    }

    fn remove(&self, dest: &Dest) {
        if let Some(worker) = self.workers.lock().remove(dest) {
            worker.retired.store(true, Ordering::SeqCst);
        }
        let _ = std::fs::remove_file(path_of(&self.shared.dir, dest));
    }

    /// Files moved aside because they could not be used. They can hold a channel's text, and clearing the channels clears them too.
    pub fn remove_leftovers(&self) {
        let Ok(files) = std::fs::read_dir(&self.shared.dir) else { return };
        for file in files.flatten().filter(|f| f.file_name().to_string_lossy().contains(".unreadable-")) {
            let _ = std::fs::remove_file(file.path());
        }
    }

    /// A worker for what a run before this one left to send. Called once, at the start; nothing starts without something to send.
    pub fn resume(&self) {
        let Ok(files) = std::fs::read_dir(&self.shared.dir) else { return };
        let config = self.shared.config.lock().clone();
        let mut known: HashMap<String, Dest> = HashMap::new();
        for ch in CHANNELS.into_iter().filter(|c| channel_on(&config, *c)) {
            known.insert(file_name(&Dest::Channel(ch)), Dest::Channel(ch));
        }
        for id in self.device_ids() {
            let dest = Dest::Device(id);
            known.insert(file_name(&dest), dest);
        }
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().to_string();
            if !name.ends_with(".json") {
                continue;
            }
            match known.get(&name) {
                Some(dest) => self.worker(dest),
                // Of a destination that is no more, if that is known: a state that could not be read says nothing of it.
                None => {
                    let known = if name.starts_with("device-") { &self.shared.devices_known } else { &self.shared.config_known };
                    if known.load(Ordering::Relaxed) {
                        let _ = std::fs::remove_file(file.path());
                    }
                }
            }
        }
    }

    fn worker(&self, dest: &Dest) {
        let mut workers = self.workers.lock();
        if !workers.contains_key(dest) {
            if let Some(worker) = spawn(dest.clone(), self.shared.clone()) {
                workers.insert(dest.clone(), worker);
            }
        }
    }

    /// Hands an entry to a destination's thread; the first one starts it. Takes no time worth mentioning: nothing here waits on a disk
    /// or a network. A full queue drops the entry (counted, shown in the settings, logged).
    pub fn enqueue(&self, dest: Dest, entry: Entry) {
        let mut workers = self.workers.lock();
        let mut entry = entry;
        // Twice at most: a thread that ended (its subscription was over, and a newer one is here) gives way to another.
        for _ in 0..2 {
            if !workers.contains_key(&dest) {
                match spawn(dest.clone(), self.shared.clone()) {
                    Some(worker) => {
                        workers.insert(dest.clone(), worker);
                    }
                    None => break,
                }
            }
            let Some(worker) = workers.get(&dest) else { break };
            match worker.tx.try_send(entry) {
                Ok(()) => return,
                Err(mpsc::TrySendError::Full(_)) => {
                    tracing::warn!("notify: the queue is full, a message was dropped");
                    break;
                }
                Err(mpsc::TrySendError::Disconnected(back)) => {
                    workers.remove(&dest);
                    entry = back;
                }
            }
        }
        self.shared.dropped.fetch_add(1, Ordering::Relaxed);
    }

    /// Whether a thread was ever started and still stands.
    #[cfg(test)]
    pub fn started(&self) -> bool {
        !self.workers.lock().is_empty()
    }

    /// The signing key's public half. Made on first need when `create`: a read that failed, or a value that is no key, is an error and
    /// nothing is written over it, since every phone's subscription hangs on this key.
    pub fn vapid_public(&self, create: bool) -> Result<Option<String>, String> {
        Ok(self.shared.vapid(create)?.map(|v| v.public_key()))
    }

    /// The key went with a reset: it is read again, and made again, when next needed.
    pub fn forget_vapid(&self) {
        *self.shared.vapid.lock() = None;
    }

    /// Sends a test alert to one phone now, on the caller's thread.
    pub fn test_device(&self, device: &str) -> Result<(), String> {
        let out = self.shared.push_to(device, &Alert::test());
        self.shared.finished(&Dest::Device(device.to_string()), &out);
        out.map_err(|f| f.reason)
    }

    /// To every phone with alerts on; fine when one got it.
    pub fn test_devices(&self) -> Result<(), String> {
        let ids = self.device_ids();
        if ids.is_empty() {
            return Err("no phone has alerts on".into());
        }
        let results: Vec<_> = ids.iter().map(|id| self.test_device(id)).collect();
        if results.iter().any(Result::is_ok) {
            return Ok(());
        }
        results.into_iter().next().unwrap_or(Ok(()))
    }
}

/// What an attempt says to a phone.
struct Alert {
    wording: Wording,
    /// The session's link, and whether a tap should say so when the wait has gone by then.
    link: Option<(String, bool)>,
    /// What makes it this alert and no other. Only another alert about the very same thing may take its place, on the way or in the tray.
    about: String,
    high: bool,
}

impl Alert {
    fn test() -> Alert {
        // Each its own: a second press must show something, with the first still in the tray.
        Alert { wording: Wording::Test, link: None, about: format!("test|{}", now_millis()), high: true }
    }

    fn of(attempt: &Attempt) -> Alert {
        match attempt {
            Attempt::One(e) => {
                let kind = Kind::parse(&e.kind).unwrap_or(Kind::Waiting);
                let about = e.session.as_ref().map_or_else(|| e.id.clone(), |s| format!("{}|{s}|{}", kind.code(), e.wait.unwrap_or(0)));
                Alert { wording: Wording::Kind(kind), link: e.session.clone().map(|s| (s, kind.waits())), about, high: kind.urgent() }
            }
            // A summary is its own too: one from a second outage must not take the place of one not read yet. The same on every try of it.
            Attempt::Together { ids, need_you, updates, urgent, .. } => Alert {
                wording: wording_of(*need_you, *updates),
                link: None,
                about: format!("summary|{}", ids.first().map_or("", String::as_str)),
                high: *urgent,
            },
        }
    }
}

impl Shared {
    fn test_base(&self) -> Option<String> {
        #[cfg(test)]
        if let Some(base) = self.test_base.lock().clone() {
            return Some(base);
        }
        push_test_base()
    }

    fn vapid(&self, create: bool) -> Result<Option<Arc<Vapid>>, String> {
        let mut cached = self.vapid.lock();
        if let Some(v) = cached.as_ref() {
            return Ok(Some(v.clone()));
        }
        let key = match self.secrets.read(K_VAPID).map_err(|_| "the credential store did not answer".to_string())? {
            Some(text) => Vapid::from_hex(&text).ok_or("the signing key in the credential store is damaged: reset phone access")?,
            None if create => {
                let key = Vapid::generate().map_err(str::to_string)?;
                self.secrets.set(K_VAPID, &key.to_hex()).map_err(|_| "the credential store would not keep the signing key".to_string())?;
                key
            }
            None => return Ok(None),
        };
        let key = Arc::new(key);
        *cached = Some(key.clone());
        Ok(Some(key))
    }

    fn exists(&self, dest: &Dest) -> bool {
        match dest {
            Dest::Channel(c) => channel_on(&self.config.lock(), *c),
            Dest::Device(id) => self.devices.lock().contains_key(id),
        }
    }

    /// The device has a subscription, and not at the address that ended.
    fn has_moved_on(&self, dest: &Dest, endpoint: Option<&str>) -> bool {
        let Dest::Device(id) = dest else { return false };
        let now = self.devices.lock().get(id).map(|t| t.endpoint.clone());
        now.is_some() && now.as_deref() != endpoint
    }

    /// Whether what an entry says is still so. Only someone waiting can stop being so: what happened, happened.
    fn still_so(&self, e: &Entry) -> bool {
        match Kind::parse(&e.kind) {
            Some(Kind::Waiting) => e.session.as_deref().is_none_or(|s| self.host.still_waiting(s, e.wait)),
            Some(Kind::Approval) => e.task.as_ref().is_none_or(|(run, task)| self.host.task_waiting(run, task)),
            _ => true,
        }
    }

    fn attempt(&self, dest: &Dest, attempt: &Attempt) -> Result<(), Failure> {
        match dest {
            Dest::Channel(ch) => {
                let c = self.config.lock().clone();
                let payload = match attempt {
                    Attempt::One(e) => Payload { id: Some(e.id.clone()), ..e.payload.clone().ok_or("nothing to say")? },
                    Attempt::Together { ids, need_you, updates, urgent, latest } => together_payload(ids, *need_you, *updates, *urgent, *latest),
                };
                deliver(self.secrets.as_ref(), &c, *ch, &payload, &telegram_base())
            }
            Dest::Device(id) => self.push_to(id, &Alert::of(attempt)),
        }
    }

    /// One request to a phone's push service. The address is checked here again: the record comes from a file, its address from a phone.
    fn push_to(&self, device: &str, alert: &Alert) -> Result<(), Failure> {
        let target = self.devices.lock().get(device).cloned().ok_or("this phone has no alerts on")?;
        self.push_to_target(device, &target, alert).map_err(|mut f| {
            if f.over {
                f.over_endpoint = Some(target.endpoint.clone());
            }
            f
        })
    }

    fn push_to_target(&self, device: &str, target: &Target, alert: &Alert) -> Result<(), Failure> {
        if !web_push::endpoint_allowed(&target.endpoint, self.test_base().as_deref()) {
            return Err(Failure::gone("not a push service"));
        }
        let auth = match self.secrets.read(&auth_key(device)) {
            Ok(Some(a)) => a,
            Ok(None) => return Err(Failure::gone("the subscription's secret is gone")),
            Err(_) => return Err(Failure::again("the credential store did not answer")),
        };
        // Never made here: every subscription hangs on the key it was made under, so with none the phone has to subscribe again.
        let vapid = self.vapid(false).map_err(Failure::again)?.ok_or_else(|| Failure::gone("no signing key"))?;
        let base = match &target.origin {
            Some(origin) => format!("{origin}/"),
            None => self.host.phone_url().ok_or_else(|| Failure::again("no address for the phone page"))?,
        };
        let navigate = match &alert.link {
            Some((session, waits)) => deep_link(&base, session).map(|l| if *waits { format!("{l}&a=1") } else { l }).unwrap_or_else(|| base.clone()),
            None => base.clone(),
        };
        let topic = web_push::topic(&alert.about);
        let body = web_push::alert_json(alert.wording, &target.lang, &navigate, &topic);
        if body.len() > web_push::MAX_ALERT_BYTES {
            return Err("too long".into());
        }
        let sealed = web_push::encrypt(&body, &target.p256dh, &auth).map_err(Failure::gone)?;
        let authorization = vapid.authorization(&target.endpoint, now_millis() / 1000).ok_or("no address")?;
        let urgency = if alert.high { "high" } else { "normal" };
        let headers = [("Authorization", authorization.as_str()), ("TTL", web_push::TTL_SECONDS), ("Urgency", urgency), ("Topic", topic.as_str()), ("Content-Encoding", "aes128gcm")];
        let out = super::post(&target.endpoint, &headers, "application/octet-stream", &sealed);
        if let Err(f) = &out {
            if f.subscription_over() {
                return Err(Failure::gone(f.reason.clone()));
            }
        }
        out
    }

    /// What became of an attempt, in the status list; a subscription that is over stops being one.
    fn finished(&self, dest: &Dest, out: &Result<(), Failure>) {
        let ch = match dest {
            Dest::Channel(c) => *c,
            Dest::Device(_) => Channel::WebPush,
        };
        record(&self.status, ch, &out.as_ref().map(|_| ()).map_err(|f| f.reason.clone()));
        if let (Dest::Device(id), Err(f)) = (dest, out) {
            if f.over {
                self.host.subscription_gone(id, f.over_endpoint.as_deref());
            }
        }
    }
}

fn wording_of(need_you: usize, updates: usize) -> Wording {
    match (need_you, updates) {
        (0, m) => Wording::Updates(m),
        (n, 0) => Wording::NeedYou(n),
        (n, m) => Wording::NeedYouAndUpdates(n, m),
    }
}

/// The message for a channel that stands for several: `status` is `needs_you` when someone waits, `failed` when something in it failed, else `summary`.
fn together_payload(ids: &[String], need_you: usize, updates: usize, urgent: bool, latest: u64) -> Payload {
    let status = if need_you > 0 {
        "needs_you"
    } else if urgent {
        "failed"
    } else {
        "summary"
    };
    Payload { status: status.into(), project: None, session: None, summary: web_push::sentence(wording_of(need_you, updates), "en"), duration_s: None, id: ids.first().cloned(), screen: None, link: None, at: latest }
}

fn spawn(dest: Dest, shared: Arc<Shared>) -> Option<Worker> {
    let (tx, rx) = mpsc::sync_channel::<Entry>(QUEUE);
    let _ = std::fs::create_dir_all(&shared.dir);
    let retired = Arc::new(AtomicBool::new(false));
    let its = retired.clone();
    match std::thread::Builder::new().name("kelpie-notify".into()).spawn(move || run(dest, rx, shared, its)) {
        Ok(_) => Some(Worker { tx, retired }),
        Err(e) => {
            // Nothing is stored: the next message tries again.
            tracing::warn!("notify worker: {e}");
            None
        }
    }
}

/// A number below `n`, from the OS generator.
fn dice(n: u64) -> u64 {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    u64::from_le_bytes(bytes[..8].try_into().expect("8 of 16 bytes")) % n.max(1)
}

/// A destination's thread: it waits for the next entry or the next attempt, whichever is first, and ends when the destination goes.
fn run(dest: Dest, rx: mpsc::Receiver<Entry>, shared: Arc<Shared>, retired: Arc<AtomicBool>) {
    let mut outbox = Outbox::open(&path_of(&shared.dir, &dest));
    loop {
        if retired.load(Ordering::SeqCst) {
            return;
        }
        // Every entry that came in, before looking at what to do.
        while let Ok(entry) = rx.try_recv() {
            shared.dropped.fetch_add(outbox.add(entry), Ordering::Relaxed);
        }
        let now = now_millis();
        if outbox.due(now) {
            if !shared.exists(&dest) {
                outbox.discard();
                return;
            }
            let lost = outbox.prune(now, &|e| shared.still_so(e));
            shared.dropped.fetch_add(lost, Ordering::Relaxed);
        }
        let attempt = match outbox.pick(now) {
            Pick::Idle => match rx.recv() {
                Ok(entry) => {
                    shared.dropped.fetch_add(outbox.add(entry), Ordering::Relaxed);
                    continue;
                }
                Err(_) => break,
            },
            Pick::Wait(wait) => match rx.recv_timeout(wait.min(Duration::from_secs(60))) {
                Ok(entry) => {
                    shared.dropped.fetch_add(outbox.add(entry), Ordering::Relaxed);
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
            Pick::Go(attempt) => attempt,
        };
        let out = shared.attempt(&dest, &attempt);
        shared.finished(&dest, &out);
        // Taken away during the request: what came of it changes no file.
        if retired.load(Ordering::SeqCst) {
            return;
        }
        match out {
            Ok(()) => outbox.sent(&attempt),
            // A late answer about an address the phone has since left: what it has to send goes to the new one.
            Err(f) if f.over && shared.has_moved_on(&dest, f.over_endpoint.as_deref()) => outbox.failed(now_millis(), None, &mut dice),
            Err(f) if f.over => {
                outbox.discard();
                return;
            }
            // A channel's receiver acts on each send, so one that may have arrived is not repeated without end. A phone's repeats take each other's place, under one `Topic` and `tag`.
            Err(f) if matches!(dest, Dest::Channel(_)) && (f.unsure || (dest == Dest::Channel(Channel::Webhook) && f.webhook_may_have_acted())) => {
                let lost = outbox.unsure(&attempt, now_millis(), Some(f.retry_after.unwrap_or_default().max(SECOND_SEND_AFTER)), &mut dice);
                shared.dropped.fetch_add(lost, Ordering::Relaxed);
            }
            Err(f) if f.retryable() => outbox.failed(now_millis(), f.retry_after, &mut dice),
            Err(_) => {
                let lost = outbox.refused(&attempt);
                shared.dropped.fetch_add(lost, Ordering::Relaxed);
            }
        }
    }
    // The sender was let go: the destination went, or this daemon is ending and what is on disk is for the next.
    if !retired.load(Ordering::SeqCst) && !shared.exists(&dest) {
        outbox.discard();
    }
}

#[cfg(test)]
mod tests;
