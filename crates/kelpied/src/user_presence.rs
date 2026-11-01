//! The person at the computer proves they are there before a phone-access decision that raises access takes effect: Windows
//! Hello, else the Windows password of the signed-in account; on macOS Touch ID or the login password; on Linux polkit, whose own
//! agent asks in its own words ([`polkit`]), so there the app's approval dialog is where the person sees which phone this is about.
//!
//! This is the gate, and it runs in the daemon because whoever holds the user token can impersonate anything else here:
//! `Role::App` (`peer_identity`) only decides who may raise the prompt. A forged app gains a prompt whose text the daemon wrote
//! from its own records, and the decision takes effect only if the OS says the person passed.
//!
//! A prompt takes the person's time, so it runs on a thread of its own and the caller's connection keeps working
//! (`Daemon::after_verifying` replies later). One prompt at a time ([`Presence::begin`]), for at most [`PROMPT_LIMIT`]: past that
//! the caller is answered with an error and whatever the abandoned prompt still says is ignored. A new prompt waits until an abandoned
//! one has left the screen, so what the person sees is never a stack of dialogs of which one is live. Three refusals in a few minutes,
//! or three prompts left unanswered in 20, stop new prompts for a while, so a program cannot keep asking until someone gives in.
//!
//! What a person is told here is English; the app translates these fixed messages (`locales/vi/backend.json`, `errorText`).
//!
//! What a prompt says comes from the table in [`Prompt::text`], in the language the app names; a caller supplies no text.
//!
//! Test seam: a debug build reads `KELPIE_TEST_USER_PRESENCE` (`approve` or `deny`) instead of prompting, so end-to-end tests
//! never raise a real prompt; a release build has no such path. Unit tests answer through [`Presence::inject`], one daemon at
//! a time, and a test that injected nothing is refused: `cargo test` can never raise a real prompt.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::mobile::Permission;

/// Refused while a prompt is on screen.
const ONE_AT_A_TIME: &str = "Kelpie is already asking you to verify: answer that prompt first.";
/// Refused while a prompt past its time is still on screen (a password dialog cannot be taken back).
const CLOSE_THE_EARLIER: &str = "An earlier verification dialog is still open: close the earlier dialog first. If there is no such dialog, restart Kelpie's background service.";
const NOT_IN_TIME: &str = "The verification was not answered in time.";
const TRY_AGAIN: &str = "The verification failed: please try again.";
const PAUSED: &str = "You refused or did not answer the last 3 verifications, so Kelpie will not ask again for a few minutes. If you did not start them, turn off phone access and revoke all (that needs no verification).";
/// How long a prompt holds the screen: a little under the 300 s the app waits for its answer.
pub const PROMPT_LIMIT: Duration = Duration::from_secs(280);
/// Refusals within `REFUSAL_WINDOW` that stop new prompts for `PAUSE`.
const REFUSALS: usize = 3;
const REFUSAL_WINDOW: Duration = Duration::from_secs(300);
/// A prompt left unanswered takes `PROMPT_LIMIT` to count, so three of them need a longer window.
const TIMEOUT_WINDOW: Duration = Duration::from_secs(1200);
const PAUSE: Duration = Duration::from_secs(300);
/// A prompt up for longer than this is worth a line in the audit log: nothing else can be asked meanwhile.
const STUCK_AFTER: Duration = Duration::from_secs(600);

/// The daemon's prompts: one on screen at a time.
pub struct Presence {
    showing: Arc<AtomicBool>,
    /// A prompt's thread is running: it may have outlived its time, and its dialog may still be up.
    prompt_up: Arc<AtomicBool>,
    /// When the latest prompt went up, and whether the log has heard that it is stuck.
    prompt_since: Mutex<Option<(Instant, bool)>>,
    limit: Mutex<Duration>,
    refusals: Mutex<Refusals>,
    #[cfg(test)]
    injected: Mutex<Option<Injected>>,
}

impl Default for Presence {
    fn default() -> Self {
        Presence {
            showing: Arc::default(),
            prompt_up: Arc::default(),
            prompt_since: Mutex::default(),
            limit: Mutex::new(PROMPT_LIMIT),
            refusals: Mutex::default(),
            #[cfg(test)]
            injected: Mutex::default(),
        }
    }
}

#[derive(Default)]
struct Refusals {
    recent: VecDeque<Instant>,
    /// Prompts nobody answered.
    timeouts: VecDeque<Instant>,
    /// No new prompt until then.
    paused_until: Option<Instant>,
}

/// Why the person was not verified.
#[derive(Debug)]
pub struct Unverified {
    pub message: String,
    /// Nobody answered in time (as opposed to a refusal or a failure).
    pub timed_out: bool,
    /// This computer has no way to ask anyone: it says nothing about the person.
    pub could_not_ask: bool,
}

/// What a prompt asks of the person. Its text is the daemon's own, from its own records.
#[derive(Clone, Debug)]
pub enum Prompt {
    Pair { name: String, key: String, permission: Permission },
    Raise { name: String, permission: Permission },
    Tailscale,
    Reset,
    /// Keep unused phones paired for this many days (0: for ever).
    KeepUnused { days: u32 },
    /// Make the phones' app lock weaker.
    WeakenLock,
    /// Let this phone set up a new passkey.
    ResetLock { name: String },
}

/// The languages the daemon's prompts are written in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lang {
    En,
    Vi,
}

impl Lang {
    /// From the code the app sends (`vi`, `vi-VN`); anything else is English.
    pub fn from_code(code: Option<&str>) -> Lang {
        match code.and_then(|c| c.split(['-', '_']).next()) {
            Some(c) if c.eq_ignore_ascii_case("vi") => Lang::Vi,
            _ => Lang::En,
        }
    }
}

impl Prompt {
    /// What the prompt says. The name is a phone's own, so it stays quoted.
    pub fn text(&self, lang: Lang) -> String {
        let may = |p: Permission| match (lang, p) {
            (Lang::En, Permission::Read) => "read your sessions",
            (Lang::En, Permission::Write) => "read your sessions and type into them",
            (Lang::En, Permission::Control) => "read your sessions, type into them and take control of this computer",
            (Lang::Vi, Permission::Read) => "đọc các phiên của bạn",
            (Lang::Vi, Permission::Write) => "đọc và gõ vào các phiên của bạn",
            (Lang::Vi, Permission::Control) => "đọc, gõ vào các phiên của bạn và điều khiển máy tính này",
        };
        match (self, lang) {
            (Prompt::Pair { name, key, permission }, Lang::En) => format!("Kelpie: allow the phone {} (key {key}) to {} on this computer.", quoted(name), may(*permission)),
            (Prompt::Pair { name, key, permission }, Lang::Vi) => format!("Kelpie: cho phép điện thoại {} (khóa {key}) {} trên máy tính này.", quoted(name), may(*permission)),
            (Prompt::Raise { name, permission }, Lang::En) => format!("Kelpie: allow the phone {} to {} on this computer.", quoted(name), may(*permission)),
            (Prompt::Raise { name, permission }, Lang::Vi) => format!("Kelpie: cho phép điện thoại {} {} trên máy tính này.", quoted(name), may(*permission)),
            (Prompt::Tailscale, Lang::En) => "Kelpie: publish the phone page of this computer on your Tailscale network, so that paired phones can reach it there.".to_string(),
            (Prompt::Tailscale, Lang::Vi) => "Kelpie: công bố trang điện thoại của máy tính này trên mạng Tailscale của bạn, để các điện thoại đã ghép đôi kết nối được qua đó.".to_string(),
            (Prompt::Reset, Lang::En) => "Kelpie: reset phone access. Every paired phone is removed and the pairing keys are replaced.".to_string(),
            (Prompt::Reset, Lang::Vi) => "Kelpie: đặt lại truy cập từ điện thoại. Mọi điện thoại đã ghép đôi sẽ bị gỡ và các khóa ghép đôi được thay mới.".to_string(),
            (Prompt::KeepUnused { days: 0 }, Lang::En) => "Kelpie: keep phones that are not used paired for ever.".to_string(),
            (Prompt::KeepUnused { days: 0 }, Lang::Vi) => "Kelpie: giữ ghép đôi mãi mãi các điện thoại không được dùng.".to_string(),
            (Prompt::KeepUnused { days }, Lang::En) => format!("Kelpie: keep phones that are not used paired for {days} days."),
            (Prompt::KeepUnused { days }, Lang::Vi) => format!("Kelpie: giữ ghép đôi {days} ngày các điện thoại không được dùng."),
            (Prompt::WeakenLock, Lang::En) => "Kelpie: make the app lock on your phones weaker.".to_string(),
            (Prompt::WeakenLock, Lang::Vi) => "Kelpie: nới lỏng khóa ứng dụng trên điện thoại của bạn.".to_string(),
            (Prompt::ResetLock { name }, Lang::En) => format!("Kelpie: let the phone {} set up a new passkey for its app lock, for the next 5 minutes.", quoted(name)),
            (Prompt::ResetLock { name }, Lang::Vi) => format!("Kelpie: cho phép điện thoại {} thiết lập khóa truy cập mới cho khóa ứng dụng, trong 5 phút tới.", quoted(name)),
        }
    }
}

/// How an abandoned prompt is taken off the screen, for a prompt that can be.
#[derive(Clone, Default)]
#[cfg_attr(test, allow(dead_code))]
pub struct Cancel(Arc<Mutex<CancelState>>);

#[derive(Default)]
struct CancelState {
    fired: bool,
    action: Option<Box<dyn FnOnce() + Send>>,
}

#[cfg_attr(test, allow(dead_code))]
impl Cancel {
    /// `action` takes the prompt off the screen when the time is up (at once, if it already is).
    pub fn on_fire(&self, action: Box<dyn FnOnce() + Send>) {
        let mut state = self.0.lock();
        if state.fired {
            drop(state);
            action();
        } else {
            state.action = Some(action);
        }
    }

    fn fire(&self) {
        let action = {
            let mut state = self.0.lock();
            state.fired = true;
            state.action.take()
        };
        if let Some(action) = action {
            action();
        }
    }

    pub fn fired(&self) -> bool {
        self.0.lock().fired
    }
}

/// How a test says its verifications end.
#[cfg(test)]
type Injected = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// This caller's turn at the screen; the next may ask once it is dropped.
#[derive(Debug)]
pub struct Turn(Arc<AtomicBool>);

impl Drop for Turn {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// See `Presence::prompt_up`.
struct PromptUp(Arc<AtomicBool>);

impl Drop for PromptUp {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Presence {
    /// The caller's turn, or an error for the person: another prompt is up, or too many were refused just now.
    pub fn begin(&self) -> Result<Turn, String> {
        self.begin_at(Instant::now())
    }

    fn begin_at(&self, now: Instant) -> Result<Turn, String> {
        {
            let mut refusals = self.refusals.lock();
            match refusals.paused_until {
                Some(until) if now < until => return Err(PAUSED.to_string()),
                Some(_) => refusals.paused_until = None,
                None => {}
            }
        }
        if self.showing.swap(true, Ordering::SeqCst) {
            return Err(ONE_AT_A_TIME.to_string());
        }
        let turn = Turn(self.showing.clone());
        if self.prompt_up.load(Ordering::SeqCst) {
            return Err(CLOSE_THE_EARLIER.to_string());
        }
        Ok(turn)
    }

    /// A verification was refused, failed or was left unanswered. True when this one stops new prompts for a while.
    pub fn refused(&self, why: &Unverified) -> bool {
        !why.could_not_ask && self.refused_at(Instant::now(), why.timed_out)
    }

    fn refused_at(&self, now: Instant, timed_out: bool) -> bool {
        let mut refusals = self.refusals.lock();
        let (list, window) = if timed_out { (&mut refusals.timeouts, TIMEOUT_WINDOW) } else { (&mut refusals.recent, REFUSAL_WINDOW) };
        list.retain(|at| now.duration_since(*at) < window);
        list.push_back(now);
        if list.len() < REFUSALS {
            return false;
        }
        list.clear();
        refusals.paused_until = Some(now + PAUSE);
        true
    }

    /// True once for a prompt that has been up too long.
    pub fn newly_stuck(&self) -> bool {
        self.newly_stuck_at(Instant::now())
    }

    fn newly_stuck_at(&self, now: Instant) -> bool {
        let mut since = self.prompt_since.lock();
        match since.as_mut() {
            Some((at, noted)) if self.prompt_up.load(Ordering::SeqCst) && !*noted && now.duration_since(*at) >= STUCK_AFTER => {
                *noted = true;
                true
            }
            _ => false,
        }
    }

    /// The person passed: what they refused or left before says nothing any more.
    pub fn passed(&self) {
        let mut refusals = self.refusals.lock();
        refusals.recent.clear();
        refusals.timeouts.clear();
    }

    /// Asks the OS to verify the person and waits for the answer, at most `PROMPT_LIMIT`. `Err` with a message for the person when
    /// they cancelled, it failed or the time ran out; nothing but a verified person returns `Ok`. A prompt that outlives its
    /// time, or a panic on the way, ends here: what it says afterwards goes nowhere, so a late pass cannot make anything happen.
    /// `window` only parents the dialog: it comes from the caller, so it is checked and otherwise ignored.
    pub fn verify(&self, window: Option<i64>, reason: &str) -> Result<(), Unverified> {
        let failed = |message: &str, timed_out| Unverified { message: message.to_string(), timed_out, could_not_ask: polkit::could_not_ask(message) };
        let (answer, answered) = std::sync::mpsc::sync_channel(1);
        let cancel = Cancel::default();
        let (reason, asked) = (reason.to_string(), cancel.clone());
        self.prompt_up.store(true, Ordering::SeqCst);
        *self.prompt_since.lock() = Some((Instant::now(), false));
        // Cleared when the thread ends, however it ends, and before its answer is read: the next prompt is not held up by this one.
        let up = PromptUp(self.prompt_up.clone());
        #[cfg(test)]
        let injected = self.injected.lock().clone();
        std::thread::Builder::new()
            .name("kelpie-verify-prompt".into())
            .spawn(move || {
                #[cfg(test)]
                let result = {
                    let _ = (window, &asked);
                    match injected {
                        Some(injected) => injected(&reason),
                        None => Err("a test asked for verification without saying how it ends".to_string()),
                    }
                };
                #[cfg(not(test))]
                let result = ask_the_os(window, &reason, &asked);
                drop(up);
                let _ = answer.try_send(result);
            })
            .map_err(|e| {
                self.prompt_up.store(false, Ordering::SeqCst);
                failed(&e.to_string(), false)
            })?;
        let limit = *self.limit.lock();
        match answered.recv_timeout(limit) {
            Ok(result) => result.map_err(|message: String| failed(&message, false)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                cancel.fire();
                Err(failed(NOT_IN_TIME, true))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(failed(TRY_AGAIN, false)),
        }
    }

    /// How this daemon's verifications end in a test (the closure is told the prompt's text, and may wait).
    #[cfg(test)]
    pub fn inject(&self, answer: impl Fn(&str) -> Result<(), String> + Send + Sync + 'static) {
        *self.injected.lock() = Some(Arc::new(answer));
    }

    /// How long a prompt may take, in a test.
    #[cfg(test)]
    pub fn set_limit(&self, limit: Duration) {
        *self.limit.lock() = limit;
    }

    /// A test about something else may refuse as often as it needs.
    #[cfg(test)]
    pub fn forget_refusals(&self) {
        *self.refusals.lock() = Refusals::default();
    }
}

#[cfg(not(test))]
fn ask_the_os(window: Option<i64>, reason: &str, cancel: &Cancel) -> Result<(), String> {
    #[cfg(debug_assertions)]
    if let Ok(answer) = std::env::var("KELPIE_TEST_USER_PRESENCE") {
        return if answer == "approve" { Ok(()) } else { Err("Verification was cancelled.".into()) };
    }
    os::verify(window, reason, cancel)
}

/// The test seam in effect, for a test script to make sure before it does anything that would raise a real prompt. A release
/// build has none.
pub fn test_seam() -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var("KELPIE_TEST_USER_PRESENCE").ok()
    } else {
        None
    }
}

/// `name` as it may appear in a prompt: a phone chose it, so it stays quoted and cannot end the quote or reorder what surrounds it.
pub fn quoted(name: &str) -> String {
    let clean: String = name.chars().filter(|c| !c.is_control() && !matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2028}' | '\u{2029}' | '\u{2060}' | '\u{2066}'..='\u{2069}' | '\u{061C}' | '\u{FEFF}' | '"' | '“' | '”')).take(40).collect();
    format!("\u{201C}{clean}\u{201D}")
}

#[cfg(all(windows, not(test)))]
mod os {
    use super::windows_os::{password_prompt, valid_window};
    use super::Cancel;

    pub fn verify(window: Option<i64>, reason: &str, cancel: &Cancel) -> Result<(), String> {
        use windows::core::HSTRING;
        use windows::Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability};
        use windows::Win32::System::WinRT::IUserConsentVerifierInterop;

        // The one place that decides the parent of both dialogs. Should a parented dialog turn out to disable Kelpie's window, pass
        // `HWND::default()` here: `AllowSetForegroundWindow` in the app still lets the dialog come to the front.
        let hwnd = valid_window(window);
        let available = UserConsentVerifier::CheckAvailabilityAsync().and_then(|op| op.join()).map(|a| a == UserConsentVerifierAvailability::Available).unwrap_or(false);
        if available {
            // A desktop program asks for a window, so the prompt comes up in front of it.
            let asked = windows::core::factory::<UserConsentVerifier, IUserConsentVerifierInterop>().and_then(|interop| {
                // SAFETY: `hwnd` is a window that exists, or null; the reason is a live string.
                let op: windows_future::IAsyncOperation<UserConsentVerificationResult> = unsafe { interop.RequestVerificationForWindowAsync(hwnd, &HSTRING::from(reason))? };
                let taken_back = op.clone();
                cancel.on_fire(Box::new(move || {
                    let _ = taken_back.Cancel();
                }));
                op.join()
            });
            // Taken back because its time ran out: nothing else is put on the screen, nobody is waiting for it.
            if cancel.fired() {
                return Err(super::NOT_IN_TIME.into());
            }
            match asked {
                Ok(UserConsentVerificationResult::Verified) => return Ok(()),
                Ok(UserConsentVerificationResult::Canceled) => return Err("Verification was cancelled.".into()),
                Ok(other) => return Err(format!("Windows could not verify you ({}).", other.0)),
                // Windows Hello would not ask for this window (it belongs to another process, or there is none): the password
                // is as good a proof.
                Err(e) => tracing::warn!("Windows Hello: {}", e.message()),
            }
        }
        password_prompt(hwnd, reason)
    }
}

/// The Windows credential prompt, and the proof that what it was given belongs to the user this daemon runs as.
#[cfg(windows)]
mod windows_os {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{EqualSid, GetLengthSid, GetTokenInformation, TokenUser, PSID, TOKEN_QUERY, TOKEN_USER};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// The caller's window hint if it names a window that exists; otherwise none (the dialog then has no parent).
    #[cfg_attr(test, allow(dead_code))]
    pub fn valid_window(hint: Option<i64>) -> windows::Win32::Foundation::HWND {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::IsWindow;
        let hwnd = HWND(hint.unwrap_or(0) as isize as *mut _);
        // SAFETY: IsWindow takes any value and only answers whether it is a window.
        if !hwnd.is_invalid() && unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            hwnd
        } else {
            HWND::default()
        }
    }

    /// A security identifier, aligned as the API wants it.
    pub struct Sid(Vec<u32>);

    impl Sid {
        fn psid(&self) -> PSID {
            PSID(self.0.as_ptr() as *mut _)
        }

        #[cfg(test)]
        pub fn from_words(words: Vec<u32>) -> Sid {
            Sid(words)
        }
    }

    /// Closes a token on every path.
    struct Token(HANDLE);

    impl Drop for Token {
        fn drop(&mut self) {
            // SAFETY: a handle this module opened and closes once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// The user a token belongs to.
    pub fn token_user(token: HANDLE) -> Result<Sid, String> {
        let mut size = 0u32;
        // SAFETY: asks for the size only; the call fails by design and reports it.
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut size) };
        if size == 0 {
            return Err("Windows would not say who that account is.".into());
        }
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: the buffer holds `size` bytes, aligned for TOKEN_USER; the SID it points at lives in the same buffer.
        unsafe {
            GetTokenInformation(token, TokenUser, Some(buffer.as_mut_ptr().cast()), size, &mut size).map_err(|e| e.message())?;
            let sid = (*(buffer.as_ptr() as *const TOKEN_USER)).User.Sid;
            let length = GetLengthSid(sid) as usize;
            let mut copy = vec![0u32; length.div_ceil(4)];
            std::ptr::copy_nonoverlapping(sid.0 as *const u8, copy.as_mut_ptr() as *mut u8, length);
            Ok(Sid(copy))
        }
    }

    pub fn same_user(a: &Sid, b: &Sid) -> bool {
        // SAFETY: both come from `token_user` or `CreateWellKnownSid`: whole, valid SIDs.
        unsafe { EqualSid(a.psid(), b.psid()) }.is_ok()
    }

    /// The user this process runs as.
    pub fn process_user() -> Result<Sid, String> {
        let mut token = HANDLE::default();
        // SAFETY: the pseudo handle needs no closing; the token is closed by its guard.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(|e| e.message())?;
        token_user(Token(token).0)
    }

    /// The name as `LogonUserW` takes it, from what the prompt returned: `DOMAIN\user`, `user@domain` or a plain name (local).
    pub fn logon_names(user: &[u16], domain: &[u16]) -> (Vec<u16>, Vec<u16>) {
        use windows::core::PCWSTR;
        use windows::Win32::Security::Credentials::CredUIParseUserNameW;
        let (mut name, mut dom) = (vec![0u16; 514], vec![0u16; 338]);
        // SAFETY: a zero-terminated string from CredUI, and buffers whose lengths the slices carry.
        let parsed = unsafe { CredUIParseUserNameW(PCWSTR(user.as_ptr()), &mut name, &mut dom) };
        let (name, dom) = if parsed.is_ok() { (name, dom) } else { (user.to_vec(), domain.to_vec()) };
        let dom = if dom.first().copied().unwrap_or(0) == 0 { domain.to_vec() } else { dom };
        // No domain: `.` is this computer, unless the name is `user@domain` (which goes with a null domain, see the caller).
        let has_at = name.iter().take_while(|c| **c != 0).any(|c| *c == u16::from(b'@'));
        let dom = if dom.first().copied().unwrap_or(0) == 0 && !has_at { vec![u16::from(b'.'), 0] } else { dom };
        (name, dom)
    }

    /// The Windows credential prompt for the signed-in account, its answer checked with `LogonUserW`. The account must be the
    /// one this daemon runs as: another valid account's password proves nothing about the person this computer is signed in to.
    #[cfg_attr(test, allow(dead_code))]
    pub fn password_prompt(hwnd: windows::Win32::Foundation::HWND, reason: &str) -> Result<(), String> {
        use windows::core::{PCWSTR, PWSTR};
        use windows::Win32::Security::Credentials::{CredUIPromptForWindowsCredentialsW, CredUnPackAuthenticationBufferW, CREDUIWIN_ENUMERATE_CURRENT_USER, CREDUI_INFOW, CRED_PACK_FLAGS};
        use windows::Win32::Security::{LogonUserW, LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT};
        use windows::Win32::System::Com::CoTaskMemFree;
        use zeroize::Zeroize as _;

        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let caption = wide("Kelpie");
        let message = wide(reason);
        let info = CREDUI_INFOW {
            cbSize: std::mem::size_of::<CREDUI_INFOW>() as u32,
            hwndParent: hwnd,
            pszMessageText: PCWSTR(message.as_ptr()),
            pszCaptionText: PCWSTR(caption.as_ptr()),
            ..Default::default()
        };
        let mut package = 0u32;
        let mut out: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut out_len = 0u32;
        // SAFETY: every pointer is to a local that outlives the call; the buffer it returns is freed below.
        let r = unsafe { CredUIPromptForWindowsCredentialsW(Some(&info), 0, &mut package, None, 0, &mut out, &mut out_len, None, CREDUIWIN_ENUMERATE_CURRENT_USER) };
        if r != 0 {
            return Err("Verification was cancelled.".into());
        }
        let (mut user, mut domain, mut pass) = (vec![0u16; 514], vec![0u16; 338], vec![0u16; 514]);
        let (mut ul, mut dl, mut pl) = (user.len() as u32, domain.len() as u32, pass.len() as u32);
        // SAFETY: the buffers are as large as the lengths say.
        let unpacked = unsafe { CredUnPackAuthenticationBufferW(CRED_PACK_FLAGS(0), out, out_len, Some(PWSTR(user.as_mut_ptr())), &mut ul, Some(PWSTR(domain.as_mut_ptr())), Some(&mut dl), Some(PWSTR(pass.as_mut_ptr())), &mut pl) };
        // SAFETY: the buffer came from CredUI with this length; its secret is wiped before it is freed.
        unsafe {
            std::slice::from_raw_parts_mut(out as *mut u8, out_len as usize).zeroize();
            CoTaskMemFree(Some(out));
        }
        // What the person is told, and what the log is told.
        let checked: Result<(), (&str, String)> = (|| {
            unpacked.map_err(|e| (COULD_NOT_CHECK, e.message()))?;
            let (name, dom) = logon_names(&user, &domain);
            let dom_arg = if dom.first().copied().unwrap_or(0) == 0 { PCWSTR::null() } else { PCWSTR(dom.as_ptr()) };
            let mut token = HANDLE::default();
            // SAFETY: zero-terminated strings; the token is closed by its guard.
            unsafe { LogonUserW(PCWSTR(name.as_ptr()), dom_arg, PCWSTR(pass.as_ptr()), LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, &mut token) }.map_err(|e| (logon_refusal(e.code().0 as u32 & 0xFFFF), e.message()))?;
            let token = Token(token);
            let (them, me) = (token_user(token.0).map_err(|e| (COULD_NOT_CHECK, e))?, process_user().map_err(|e| (COULD_NOT_CHECK, e))?);
            if same_user(&them, &me) {
                Ok(())
            } else {
                Err((NOT_THIS_ACCOUNT, "that is not the account signed in here".to_string()))
            }
        })();
        pass.zeroize();
        checked.map_err(|(person, why)| {
            tracing::info!("password verification failed: {why}");
            person.to_string()
        })
    }

    const WRONG_PASSWORD: &str = "That is not the password of the account signed in to this computer.";
    const NOT_THIS_ACCOUNT: &str = "That is not the account signed in to this computer.";
    const ACCOUNT_REFUSED: &str = "Windows would not sign that account in here, so it could not confirm you. Set up Windows Hello or check the account.";
    const COULD_NOT_CHECK: &str = "Windows could not check what was entered: please try again.";

    /// What a person is told about a logon Windows refused, from its error code. Only a wrong name or password is called that;
    /// an account with a blank password, or one that may not sign in this way, has no wrong password to correct.
    #[cfg_attr(test, allow(dead_code))]
    pub fn logon_refusal(code: u32) -> &'static str {
        match code {
            1326 => WRONG_PASSWORD,                                    // ERROR_LOGON_FAILURE
            1327 | 1330 | 1331 | 1385 | 1793 | 1909 => ACCOUNT_REFUSED, // restriction (a blank password), expired, disabled, logon type not granted, account expired, locked out
            _ => COULD_NOT_CHECK,
        }
    }
}

#[cfg(all(target_os = "macos", not(test)))]
mod os {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};

    use super::Cancel;

    /// Takes the prompt off the screen from the thread that gave up on it.
    struct Context(objc2::rc::Retained<LAContext>);
    // SAFETY: only `invalidate` is called through this, the framework's way to end an evaluation under way from outside it, and it holds
    // its own reference to the object.
    unsafe impl Send for Context {}

    /// Touch ID, or the account password when there is none (`DeviceOwnerAuthentication`). There is no window to parent. A prompt past
    /// its time is taken back with `invalidate`, which ends the wait below; should it not, its answer goes nowhere.
    pub fn verify(_window: Option<i64>, reason: &str, cancel: &Cancel) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::sync_channel::<bool>(1);
        let reason = NSString::from_str(reason);
        // SAFETY: LocalAuthentication calls the reply block once, on a queue of its own.
        unsafe {
            let context = LAContext::new();
            let taken_back = Context(context.clone());
            cancel.on_fire(Box::new(move || {
                let context = taken_back;
                context.0.invalidate();
            }));
            let reply = RcBlock::new(move |ok: Bool, _error: *mut NSError| {
                let _ = tx.try_send(ok.as_bool());
            });
            context.evaluatePolicy_localizedReason_reply(LAPolicy::DeviceOwnerAuthentication, &reason, &reply);
        }
        match rx.recv() {
            Ok(true) => Ok(()),
            _ => Err("Verification was cancelled.".into()),
        }
    }
}

#[cfg(all(not(any(windows, target_os = "macos")), not(test)))]
mod os {
    use super::Cancel;

    /// polkit shows its own text: the daemon's reason cannot be passed to it, and there is no window to parent.
    pub fn verify(_window: Option<i64>, _reason: &str, cancel: &Cancel) -> Result<(), String> {
        super::polkit::verify("pkcheck", std::process::id(), cancel)
    }
}

/// Verifying the person through polkit on Linux, by running `pkcheck` as this process. It runs nothing as root, which `pkexec` would have
/// to, and says what happened in its exit code. `org.freedesktop.policykit.exec` is an action every polkit has; its default asks an
/// administrator's password, so this passes only for someone who could also have used `pkexec`. Fails closed: with no polkit,
/// no agent to ask through (a headless login) or anything unclear, nothing passes. Not platform code, so it is built and tested on every system;
/// only `os::verify` above reaches it, and only on Linux, and tests run stand-ins, never a real prompt.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
mod polkit {
    use std::io::{ErrorKind, Read};
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    use super::{Cancel, NOT_IN_TIME};

    const CANCELLED: &str = "Verification was cancelled.";
    pub const NO_POLKIT: &str = "Kelpie cannot verify you here because polkit is not installed. Install polkit and an authentication agent for your desktop, then try again.";
    pub const NO_AGENT: &str = "Kelpie cannot verify you here because no polkit authentication agent is running. Sign in to a desktop session that has one, then try again.";
    pub const NOT_ALLOWED: &str = "polkit did not allow this account. Sign in as an administrator of this computer, then try again.";
    pub const COULD_NOT_CHECK: &str = "polkit could not check this: please try again.";

    /// The most of `pkcheck`'s complaints that is kept for the log. It is told no secret: the agent asks for the password, not it.
    const STDERR_KEPT: u64 = 4096;

    /// The computer has no way to ask anyone, which is no refusal by the person.
    pub fn could_not_ask(message: &str) -> bool {
        message == NO_POLKIT || message == NO_AGENT
    }

    pub fn arguments(pid: u32) -> Vec<String> {
        ["--action-id", "org.freedesktop.policykit.exec", "--process", &pid.to_string(), "--allow-user-interaction"].map(str::to_string).to_vec()
    }

    /// `pkcheck`'s exit code (pkcheck(1)); none when a signal ended it. Only 0 passes.
    pub fn result_of(code: Option<i32>) -> Result<(), &'static str> {
        match code {
            Some(0) => Ok(()),
            Some(1) => Err(NOT_ALLOWED),
            Some(2) => Err(NO_AGENT),
            Some(3) => Err(CANCELLED),
            _ => Err(COULD_NOT_CHECK),
        }
    }

    pub fn verify(program: &str, pid: u32, cancel: &Cancel) -> Result<(), String> {
        let mut child = Command::new(program)
            .args(arguments(pid))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| if e.kind() == ErrorKind::NotFound { NO_POLKIT } else { COULD_NOT_CHECK }.to_string())?;
        let (result, said) = wait(&mut child, cancel);
        if !said.is_empty() {
            tracing::debug!("pkcheck: {said}");
        }
        result
    }

    /// Waits for the child, and says what it wrote to stderr; when the time is up it is ended, which takes its request back.
    pub fn wait(child: &mut Child, cancel: &Cancel) -> (Result<(), String>, String) {
        // On a thread: a child that fills the pipe would otherwise wait for us while we wait for it.
        let reader = child.stderr.take().map(|pipe| {
            std::thread::spawn(move || {
                let mut said = Vec::new();
                let _ = pipe.take(STDERR_KEPT).read_to_end(&mut said);
                String::from_utf8_lossy(&said).trim().to_string()
            })
        });
        let result = loop {
            if cancel.fired() {
                let _ = child.kill();
                let _ = child.wait();
                break Err(NOT_IN_TIME.to_string());
            }
            match child.try_wait() {
                Ok(Some(status)) => break result_of(status.code()).map_err(str::to_string),
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(COULD_NOT_CHECK.to_string());
                }
            }
        };
        (result, reader.and_then(|r| r.join().ok()).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_that_said_nothing_is_refused_and_never_reaches_the_os() {
        let p = Presence::default();
        assert!(p.verify(None, "x").is_err());
        p.inject(|reason| if reason == "yes" { Ok(()) } else { Err("no".into()) });
        assert!(p.verify(None, "yes").is_ok());
        assert!(p.verify(None, "other").is_err());
    }

    #[test]
    fn one_prompt_at_a_time() {
        let p = Presence::default();
        let turn = p.begin().unwrap();
        assert!(p.begin().unwrap_err().contains("already"));
        drop(turn);
        assert!(p.begin().is_ok());
    }

    #[test]
    fn a_panic_in_the_prompt_still_ends_the_wait() {
        let p = Presence::default();
        p.inject(|_| panic!("the prompt fell over"));
        assert!(p.verify(None, "x").is_err());
    }

    #[test]
    fn a_prompt_past_its_time_is_an_error_and_its_late_pass_goes_nowhere() {
        let p = Presence::default();
        p.set_limit(Duration::from_millis(50));
        let (done, finished) = std::sync::mpsc::channel();
        p.inject(move |_| {
            std::thread::sleep(Duration::from_millis(300));
            let _ = done.send(());
            Ok(())
        });
        let late = p.verify(None, "x").unwrap_err();
        assert!(late.message.contains("in time") && late.timed_out);
        // Until it has left the screen nothing else is put up, and the turn is not lost by asking.
        assert!(p.begin().unwrap_err().contains("close the earlier"));
        finished.recv_timeout(Duration::from_secs(10)).expect("the prompt ended, late, and passed");
        for _ in 0..500 {
            if p.begin().is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the prompt's end did not clear the way");
    }

    #[test]
    fn a_prompt_stuck_for_long_is_said_once() {
        let p = Presence::default();
        let t0 = Instant::now();
        assert!(!p.newly_stuck_at(t0 + STUCK_AFTER), "no prompt yet");
        p.prompt_up.store(true, Ordering::SeqCst);
        *p.prompt_since.lock() = Some((t0, false));
        assert!(!p.newly_stuck_at(t0 + STUCK_AFTER - Duration::from_secs(1)));
        assert!(p.newly_stuck_at(t0 + STUCK_AFTER));
        assert!(!p.newly_stuck_at(t0 + STUCK_AFTER + Duration::from_secs(60)), "once");
        p.prompt_up.store(false, Ordering::SeqCst);
        *p.prompt_since.lock() = Some((t0, false));
        assert!(!p.newly_stuck_at(t0 + STUCK_AFTER), "it ended");
    }

    #[test]
    fn prompts_left_unanswered_stop_new_ones_over_a_longer_window() {
        let p = Presence::default();
        let t0 = Instant::now();
        // As they really come: one each time the last ran out.
        assert!(!p.refused_at(t0, true) && !p.refused_at(t0 + PROMPT_LIMIT, true));
        assert!(p.refused_at(t0 + PROMPT_LIMIT * 2, true), "three in 20 minutes");
        assert!(p.begin_at(t0 + PROMPT_LIMIT * 2 + Duration::from_secs(1)).unwrap_err().contains("will not ask again"));
        // Far apart, they do not add up, and a pass starts over.
        let p = Presence::default();
        assert!(!p.refused_at(t0, true) && !p.refused_at(t0 + Duration::from_secs(1), true));
        assert!(!p.refused_at(t0 + TIMEOUT_WINDOW + Duration::from_secs(1), true));
        let p = Presence::default();
        assert!(!p.refused_at(t0, true) && !p.refused_at(t0 + Duration::from_secs(1), true));
        p.passed();
        assert!(!p.refused_at(t0 + Duration::from_secs(2), true));
    }

    #[test]
    fn taking_a_prompt_back_works_at_any_moment() {
        let ran = Arc::new(AtomicBool::new(false));
        let (a, b) = (Cancel::default(), ran.clone());
        a.on_fire(Box::new(move || b.store(true, Ordering::SeqCst)));
        assert!(!ran.load(Ordering::SeqCst) && !a.fired());
        a.fire();
        assert!(ran.load(Ordering::SeqCst) && a.fired());
        // The time ran out before the prompt could say how to take it back.
        ran.store(false, Ordering::SeqCst);
        let (a, b) = (Cancel::default(), ran.clone());
        a.fire();
        a.on_fire(Box::new(move || b.store(true, Ordering::SeqCst)));
        assert!(ran.load(Ordering::SeqCst));
    }

    #[test]
    fn three_refusals_in_a_few_minutes_stop_new_prompts_for_a_while() {
        let p = Presence::default();
        let t0 = Instant::now();
        assert!(!p.refused_at(t0, false) && !p.refused_at(t0 + Duration::from_secs(10), false));
        assert!(p.begin_at(t0 + Duration::from_secs(11)).is_ok(), "two are not enough");
        assert!(p.refused_at(t0 + Duration::from_secs(20), false), "the third stops them");
        let later = t0 + Duration::from_secs(21);
        let refused = p.begin_at(later).unwrap_err();
        assert!(refused.contains("will not ask again") && refused.contains("revoke all"), "{refused}");
        assert!(p.begin_at(t0 + Duration::from_secs(20) + PAUSE - Duration::from_secs(1)).is_err());
        assert!(p.begin_at(t0 + Duration::from_secs(20) + PAUSE).is_ok(), "only for a while");
        // Far apart, they do not add up; a pass starts over.
        let p = Presence::default();
        assert!(!p.refused_at(t0, false) && !p.refused_at(t0 + Duration::from_secs(1), false));
        assert!(!p.refused_at(t0 + REFUSAL_WINDOW + Duration::from_secs(1), false), "the first has aged out");
        let p = Presence::default();
        assert!(!p.refused_at(t0, false) && !p.refused_at(t0 + Duration::from_secs(1), false));
        p.passed();
        assert!(!p.refused_at(t0 + Duration::from_secs(2), false));
    }

    #[test]
    fn polkit_is_asked_about_this_process_and_may_ask_the_person() {
        assert_eq!(polkit::arguments(4321), ["--action-id", "org.freedesktop.policykit.exec", "--process", "4321", "--allow-user-interaction"]);
    }

    /// pkcheck(1): 0 authorized, 1 not, 2 no agent or no interaction, 3 dismissed, 126 bad options, 127 an error.
    #[test]
    fn only_a_clean_pass_from_polkit_passes() {
        assert_eq!(polkit::result_of(Some(0)), Ok(()));
        assert_eq!(polkit::result_of(Some(1)), Err(polkit::NOT_ALLOWED));
        assert_eq!(polkit::result_of(Some(2)), Err(polkit::NO_AGENT));
        assert_eq!(polkit::result_of(Some(3)), Err("Verification was cancelled."));
        for unclear in [Some(126), Some(127), Some(-1), None] {
            assert_eq!(polkit::result_of(unclear), Err(polkit::COULD_NOT_CHECK), "{unclear:?}");
        }
    }

    #[test]
    fn no_polkit_on_the_computer_refuses_and_says_what_to_do() {
        let refused = polkit::verify("kelpie-no-such-program", 1, &Cancel::default()).unwrap_err();
        assert_eq!(refused, polkit::NO_POLKIT);
        assert!(refused.contains("Install polkit"));
    }

    /// No polkit, or no agent, is the computer's lack: however often, it does not read "You refused" or stop new prompts.
    #[test]
    fn a_computer_that_cannot_ask_is_not_a_person_refusing() {
        for message in [polkit::NO_POLKIT, polkit::NO_AGENT] {
            let p = Presence::default();
            p.inject(move |_| Err(message.to_string()));
            for _ in 0..REFUSALS * 2 {
                let turn = p.begin().expect("never paused for this");
                let why = p.verify(None, "x").unwrap_err();
                assert!(why.could_not_ask && !why.timed_out);
                assert_eq!(why.message, message);
                assert!(!p.refused(&why), "not counted");
                drop(turn);
            }
        }
        // A person's no still counts.
        let p = Presence::default();
        p.inject(|_| Err("Verification was cancelled.".to_string()));
        let refused: Vec<bool> = (0..REFUSALS).map(|_| p.refused(&p.verify(None, "x").unwrap_err())).collect();
        assert_eq!(refused, [false, false, true]);
        assert!(p.begin().unwrap_err().contains("You refused"));
    }

    /// What it wrote to stderr is there for the log, and its exit code still decides. A stand-in: no polkit is involved.
    #[test]
    fn what_the_polkit_child_complains_of_is_kept_for_the_log() {
        let mut c = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/c", "echo no agent here 1>&2 & exit /b 2"]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", "echo no agent here >&2; exit 2"]);
            c
        };
        let mut child = c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).spawn().unwrap();
        let (result, said) = polkit::wait(&mut child, &Cancel::default());
        assert_eq!(result.unwrap_err(), polkit::NO_AGENT);
        assert_eq!(said, "no agent here");
    }

    /// A stand-in that stays until it is ended: no polkit is involved.
    fn staying_child() -> std::process::Child {
        let mut c = if cfg!(windows) {
            let mut c = std::process::Command::new("ping");
            c.args(["-n", "60", "127.0.0.1"]);
            c
        } else {
            let mut c = std::process::Command::new("sleep");
            c.arg("60");
            c
        };
        c.stdout(std::process::Stdio::null()).spawn().unwrap()
    }

    #[test]
    fn the_time_limit_ends_the_polkit_child() {
        let mut child = staying_child();
        let cancel = Cancel::default();
        let later = cancel.clone();
        let fire = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            later.fire();
        });
        let began = Instant::now();
        assert_eq!(polkit::wait(&mut child, &cancel).0.unwrap_err(), NOT_IN_TIME);
        assert!(began.elapsed() < Duration::from_secs(30), "it did not wait for the child");
        assert!(child.try_wait().unwrap().is_some(), "the child is gone");
        fire.join().unwrap();
    }

    #[test]
    fn a_prompt_is_in_the_language_asked_for_and_otherwise_in_english() {
        let prompts = [
            Prompt::Pair { name: "Pixel".into(), key: "AAAA BBBB".into(), permission: Permission::Write },
            Prompt::Raise { name: "Pixel".into(), permission: Permission::Control },
            Prompt::Tailscale,
            Prompt::Reset,
            Prompt::KeepUnused { days: 0 },
            Prompt::KeepUnused { days: 120 },
            Prompt::WeakenLock,
            Prompt::ResetLock { name: "Pixel".into() },
        ];
        for p in &prompts {
            let (en, vi) = (p.text(Lang::En), p.text(Lang::Vi));
            assert_ne!(en, vi, "{p:?}");
            assert!(en.starts_with("Kelpie: ") && vi.starts_with("Kelpie: "), "{p:?}");
            assert!(!en.chars().any(|c| matches!(c, 'đ' | 'ử' | 'ọ')), "{en}");
            assert!(!vi.is_ascii(), "{vi}");
        }
        let (en, vi) = (prompts[0].text(Lang::En), prompts[0].text(Lang::Vi));
        for text in [&en, &vi] {
            assert!(text.contains("\u{201C}Pixel\u{201D}") && text.contains("AAAA BBBB"), "the name in its quotes and the key stay: {text}");
        }
        assert!(en.contains("type into them") && vi.contains("gõ"));
        assert!(prompts[5].text(Lang::Vi).contains("120") && prompts[5].text(Lang::En).contains("120"));
        for (code, lang) in [(Some("vi"), Lang::Vi), (Some("VI"), Lang::Vi), (Some("vi-VN"), Lang::Vi), (Some("en"), Lang::En), (Some("fr"), Lang::En), (Some("vietnam"), Lang::En), (Some(""), Lang::En), (None, Lang::En)] {
            assert_eq!(Lang::from_code(code), lang, "{code:?}");
        }
    }

    #[test]
    fn a_name_cannot_end_its_quote_or_turn_the_text_around() {
        assert_eq!(quoted("Pixel"), "\u{201C}Pixel\u{201D}");
        assert_eq!(quoted("a\"b\u{201D} is safe\u{202E}\n"), "\u{201C}ab is safe\u{201D}");
        assert_eq!(quoted("a\u{2028}b\u{2029}c\u{061C}d\u{FEFF}e\u{2060}f"), "\u{201C}abcdef\u{201D}", "no forced line break, nothing invisible");
        assert_eq!(quoted(&"x".repeat(100)).chars().count(), 42);
    }

    #[cfg(windows)]
    #[test]
    fn the_password_must_be_the_signed_in_users() {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Security::{CreateWellKnownSid, WinLocalSystemSid, PSID};
        let me = windows_os::process_user().unwrap();
        assert!(windows_os::same_user(&me, &windows_os::process_user().unwrap()), "the same account");
        // Another account's token: its user differs from this process's, which is the whole check.
        let mut other = vec![0u32; 32];
        let mut size = (other.len() * 4) as u32;
        // SAFETY: the buffer is as large as `size` says.
        unsafe { CreateWellKnownSid(WinLocalSystemSid, None, Some(PSID(other.as_mut_ptr() as *mut _)), &mut size) }.unwrap();
        assert!(!windows_os::same_user(&me, &windows_os::Sid::from_words(other)), "another account");
        // The window hint is only a hint: anything that is not a window is no window.
        assert_eq!(windows_os::valid_window(None), HWND::default());
        assert_eq!(windows_os::valid_window(Some(0x7fff_fff0)), HWND::default());
    }

    /// What the prompt returns, as `LogonUserW` takes it. No prompt here: the name parser is a plain function of the OS.
    #[cfg(windows)]
    #[test]
    fn the_account_name_is_taken_apart_for_the_logon() {
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let text = |w: &[u16]| String::from_utf16_lossy(&w[..w.iter().position(|c| *c == 0).unwrap_or(w.len())]);
        let names = |user: &str, domain: &str| {
            let (n, d) = windows_os::logon_names(&wide(user), &wide(domain));
            (text(&n), text(&d))
        };
        assert_eq!(names("HOST\\alice", ""), ("alice".to_string(), "HOST".to_string()));
        assert_eq!(names("MicrosoftAccount\\me@outlook.com", ""), ("me@outlook.com".to_string(), "MicrosoftAccount".to_string()));
        assert_eq!(names("alice", ""), ("alice".to_string(), ".".to_string()), "a plain name is a local account");
        assert_eq!(names("alice", "HOST"), ("alice".to_string(), "HOST".to_string()), "the domain the prompt gave is used when the name has none");
        assert_eq!(names("me@corp.example", ""), ("me@corp.example".to_string(), String::new()), "a UPN goes to the logon whole, with no domain");
    }

    /// Only a wrong password is called a wrong password.
    #[cfg(windows)]
    #[test]
    fn a_refused_logon_says_what_is_wrong() {
        assert!(windows_os::logon_refusal(1326).contains("password"));
        for restricted in [1327, 1330, 1331, 1385, 1793, 1909] {
            assert!(!windows_os::logon_refusal(restricted).contains("password"), "{restricted}");
        }
        assert!(!windows_os::logon_refusal(5).contains("password"));
    }
}
