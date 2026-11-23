//! UI Automation: a window's elements with what `tree` needs of each, read in one request, and the actions on one element.
//!
//! The whole control view of a window comes back from one cached request, so a read is one round trip to the app however many elements it
//! has. An action is sent on a worker thread and waited for a few seconds: pressing a button that opens a modal dialog does not return
//! until the dialog closes in some apps, and an agent must not hang on that.

use std::sync::mpsc;
use std::time::Duration;

use windows::core::{BSTR, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayGetVartype, SafeArrayUnaccessData,
};
use windows::Win32::System::Variant::{VT_BSTR, VT_I4};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, ExpandCollapseState_Collapsed, ExpandCollapseState_Expanded,
    ExpandCollapseState_PartiallyExpanded, IUIAutomation, IUIAutomationCacheRequest,
    IUIAutomationElement, IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern,
    IUIAutomationLegacyIAccessiblePattern, IUIAutomationScrollItemPattern,
    IUIAutomationSelectionItemPattern, IUIAutomationTogglePattern, IUIAutomationValuePattern,
    ToggleState_Indeterminate, ToggleState_On, TreeScope_Subtree, UIA_BoundingRectanglePropertyId,
    UIA_ControlTypePropertyId, UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_ExpandCollapsePatternId, UIA_HasKeyboardFocusPropertyId, UIA_InvokePatternId,
    UIA_IsEnabledPropertyId, UIA_IsExpandCollapsePatternAvailablePropertyId,
    UIA_IsInvokePatternAvailablePropertyId, UIA_IsOffscreenPropertyId, UIA_IsPasswordPropertyId,
    UIA_IsSelectionItemPatternAvailablePropertyId, UIA_IsTogglePatternAvailablePropertyId,
    UIA_IsValuePatternAvailablePropertyId, UIA_LegacyIAccessiblePatternId, UIA_NamePropertyId,
    UIA_RuntimeIdPropertyId, UIA_ScrollItemPatternId, UIA_SelectionItemIsSelectedPropertyId,
    UIA_SelectionItemPatternId, UIA_TogglePatternId, UIA_ToggleToggleStatePropertyId,
    UIA_ValueIsReadOnlyPropertyId, UIA_ValuePatternId, UIA_ValueValuePropertyId, UIA_PROPERTY_ID,
};

use crate::tree::{
    clean, role, Action, Node, MAX_DEPTH, MAX_ID_PARTS, MAX_NAME, MAX_VALUE, MAX_WALK, STALE,
};

/// How long an action is waited for before the agent is told it was sent and not yet answered.
const ACT_WAIT: Duration = Duration::from_secs(5);

/// Everything `node` reads, so that reading it costs no further round trip.
const CACHED: &[UIA_PROPERTY_ID] = &[
    UIA_RuntimeIdPropertyId,
    UIA_ControlTypePropertyId,
    UIA_NamePropertyId,
    UIA_BoundingRectanglePropertyId,
    UIA_IsEnabledPropertyId,
    UIA_IsOffscreenPropertyId,
    UIA_IsPasswordPropertyId,
    UIA_HasKeyboardFocusPropertyId,
    UIA_IsInvokePatternAvailablePropertyId,
    UIA_IsValuePatternAvailablePropertyId,
    UIA_IsTogglePatternAvailablePropertyId,
    UIA_IsExpandCollapsePatternAvailablePropertyId,
    UIA_IsSelectionItemPatternAvailablePropertyId,
    UIA_ValueValuePropertyId,
    UIA_ValueIsReadOnlyPropertyId,
    UIA_ToggleToggleStatePropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_SelectionItemIsSelectedPropertyId,
];

fn said(e: windows::core::Error) -> String {
    format!("UI Automation: {}", e.message())
}

pub struct Uia {
    auto: IUIAutomation,
    request: IUIAutomationCacheRequest,
}

impl Uia {
    pub fn new() -> Result<Uia, String> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(said)?;
            let auto: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).map_err(said)?;
            let request = auto.CreateCacheRequest().map_err(said)?;
            request.SetTreeScope(TreeScope_Subtree).map_err(said)?;
            request
                .SetTreeFilter(&auto.ControlViewCondition().map_err(said)?)
                .map_err(said)?;
            for p in CACHED {
                request.AddProperty(*p).map_err(said)?;
            }
            Ok(Uia { auto, request })
        }
    }

    /// The window's elements in document order, each with its depth. `visit` returns false to stop.
    fn walk(
        &self,
        h: HWND,
        mut visit: impl FnMut(&IUIAutomationElement, u32) -> bool,
    ) -> Result<(), String> {
        let root = unsafe {
            self.auto
                .ElementFromHandleBuildCache(h, &self.request)
                .map_err(said)?
        };
        let mut seen = 0usize;
        fn down(
            el: &IUIAutomationElement,
            depth: u32,
            seen: &mut usize,
            visit: &mut dyn FnMut(&IUIAutomationElement, u32) -> bool,
        ) -> bool {
            *seen += 1;
            if !visit(el, depth) {
                return false;
            }
            if depth >= MAX_DEPTH {
                return true;
            }
            // No children comes back as an error (a null array).
            let Ok(children) = (unsafe { el.GetCachedChildren() }) else {
                return true;
            };
            let n = unsafe { children.Length() }.unwrap_or(0);
            for i in 0..n {
                if *seen >= MAX_WALK {
                    return false;
                }
                let Ok(child) = (unsafe { children.GetElement(i) }) else {
                    continue;
                };
                if !down(&child, depth + 1, seen, visit) {
                    return false;
                }
            }
            true
        }
        down(&root, 0, &mut seen, &mut visit);
        Ok(())
    }

    /// Every element of the window as `tree` wants it, before pruning. `origin` is the window's frame, so that `at` is in the picture's
    /// pixels.
    pub fn nodes(&self, h: HWND, origin: (i32, i32)) -> Result<Vec<Node>, String> {
        let mut out = Vec::new();
        self.walk(h, |el, depth| {
            out.push(node(el, depth, origin));
            true
        })?;
        Ok(out)
    }

    /// Do one thing to the element `id` names. What comes back is what to tell the agent.
    pub fn act(&self, h: HWND, id: &[i32], action: Action) -> Result<String, String> {
        let mut found: Option<IUIAutomationElement> = None;
        self.walk(h, |el, _| {
            if runtime_id(el) == id {
                found = Some(el.clone());
                return false;
            }
            true
        })?;
        let el = Shared(found.ok_or(STALE)?);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let el = el;
            super::desk::rejoin();
            // This thread joins the process's multithreaded apartment, where the element lives.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            let _ = tx.send(unsafe { perform(&el.0, &action) });
        });
        match rx.recv_timeout(ACT_WAIT) {
            Ok(Ok(done)) => Ok(done.to_string()),
            Ok(Err(e)) => Err(e),
            Err(_) => Ok(
                "sent; the app has not answered yet (it may be showing a dialog: list the windows)"
                    .into(),
            ),
        }
    }
}

/// An element is used from the worker thread of `act`; both threads are in the multithreaded apartment, where that is allowed.
struct Shared(IUIAutomationElement);
unsafe impl Send for Shared {}

unsafe fn perform(el: &IUIAutomationElement, action: &Action) -> Result<&'static str, String> {
    let unsupported =
        |what: &str| format!("this element does not take {what}: read the tree for what it takes");
    match action {
        Action::Invoke => {
            if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            {
                p.Invoke().map_err(said)?;
            } else if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                UIA_ExpandCollapsePatternId,
            ) {
                // Something that opens and closes (a web button with `aria-expanded`, a menu button) offers no invoke: pressing it is
                // opening it when it is closed and closing it when it is open. Its default action, tried here before, said it was
                // done and did nothing in a web view.
                let open = p.CurrentExpandCollapseState().is_ok_and(|state| {
                    state == ExpandCollapseState_Expanded
                        || state == ExpandCollapseState_PartiallyExpanded
                });
                if open {
                    p.Collapse().map_err(said)?;
                    return Ok("invoked: it was open and is asked to close");
                }
                p.Expand().map_err(said)?;
                return Ok("invoked: it was closed and is asked to open");
            } else {
                // Older controls offer only their default action.
                let p = el
                    .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                        UIA_LegacyIAccessiblePatternId,
                    )
                    .map_err(|_| unsupported("invoke"))?;
                p.DoDefaultAction().map_err(said)?;
            }
            Ok("invoked")
        }
        Action::Set(text) => {
            if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
                p.SetValue(&BSTR::from(text.as_str())).map_err(said)?;
                // Some elements take the call and keep their value (a drop-down list does). What a password field holds is not read.
                let password = el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false);
                if !password {
                    // A web view applies the value a moment later (seen: an immediate read gave the old one), so it is read for half a second.
                    let mut took = false;
                    for _ in 0..10 {
                        took = !p.CurrentValue().is_ok_and(|now| now != *text);
                        if took {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    if !took {
                        return Err("not set: the element kept its value; it may take one another way (a list: expand it and select an item)".into());
                    }
                }
            } else {
                let p = el
                    .GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                        UIA_LegacyIAccessiblePatternId,
                    )
                    .map_err(|_| unsupported("set"))?;
                let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
                p.SetValue(PCWSTR(wide.as_ptr())).map_err(said)?;
            }
            Ok("set")
        }
        Action::Toggle => {
            el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                .map_err(|_| unsupported("toggle"))?
                .Toggle()
                .map_err(said)?;
            Ok("toggled")
        }
        Action::Expand | Action::Collapse => {
            let p = el
                .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                    UIA_ExpandCollapsePatternId,
                )
                .map_err(|_| unsupported("expand"))?;
            if *action == Action::Expand {
                p.Expand().map_err(said)?;
                Ok("expanded")
            } else {
                p.Collapse().map_err(said)?;
                Ok("collapsed")
            }
        }
        Action::Select => {
            el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId)
                .map_err(|_| unsupported("select"))?
                .Select()
                .map_err(said)?;
            Ok("selected")
        }
        Action::Focus => {
            el.SetFocus().map_err(said)?;
            Ok("focused")
        }
        Action::Show => {
            el.GetCurrentPatternAs::<IUIAutomationScrollItemPattern>(UIA_ScrollItemPatternId)
                .map_err(|_| unsupported("show"))?
                .ScrollIntoView()
                .map_err(said)?;
            Ok("scrolled into view")
        }
    }
}

fn truth(el: &IUIAutomationElement, p: UIA_PROPERTY_ID) -> bool {
    unsafe { el.GetCachedPropertyValue(p) }
        .ok()
        .and_then(|v| bool::try_from(&v).ok())
        .unwrap_or(false)
}

fn number(el: &IUIAutomationElement, p: UIA_PROPERTY_ID) -> Option<i32> {
    unsafe { el.GetCachedPropertyValue(p) }
        .ok()
        .and_then(|v| i32::try_from(&v).ok())
}

fn text(el: &IUIAutomationElement, p: UIA_PROPERTY_ID) -> Option<String> {
    let v = unsafe { el.GetCachedPropertyValue(p) }.ok()?;
    // The crate's own conversion to a string needs structured storage, which nothing else here uses: the field is read as it is.
    unsafe {
        let inner = &v.Anonymous.Anonymous;
        (inner.vt == VT_BSTR).then(|| inner.Anonymous.bstrVal.to_string())
    }
}

/// The element's runtime id; empty when it gives none or one too long to be an id.
fn runtime_id(el: &IUIAutomationElement) -> Vec<i32> {
    unsafe {
        let Ok(sa) = el.GetRuntimeId() else {
            return Vec::new();
        };
        if sa.is_null() {
            return Vec::new();
        }
        let mut out = Vec::new();
        // The id is read as 32-bit integers only when the array says it holds them, in one dimension.
        let ints = SafeArrayGetDim(sa) == 1 && SafeArrayGetVartype(sa).is_ok_and(|t| t == VT_I4);
        if let (true, Ok(lo), Ok(hi)) = (ints, SafeArrayGetLBound(sa, 1), SafeArrayGetUBound(sa, 1))
        {
            let n = (i64::from(hi) - i64::from(lo) + 1).max(0) as usize;
            let mut data = std::ptr::null_mut();
            if n > 0 && n <= MAX_ID_PARTS && SafeArrayAccessData(sa, &mut data).is_ok() {
                out.extend_from_slice(std::slice::from_raw_parts(data as *const i32, n));
                let _ = SafeArrayUnaccessData(sa);
            }
        }
        let _ = SafeArrayDestroy(sa);
        out
    }
}

fn node(el: &IUIAutomationElement, depth: u32, origin: (i32, i32)) -> Node {
    let cached_flag =
        |get: windows::core::Result<windows::core::BOOL>| get.map(|b| b.as_bool()).unwrap_or(false);
    let password = cached_flag(unsafe { el.CachedIsPassword() });
    let (invoke, value, toggle, expand, select) = (
        truth(el, UIA_IsInvokePatternAvailablePropertyId),
        truth(el, UIA_IsValuePatternAvailablePropertyId),
        truth(el, UIA_IsTogglePatternAvailablePropertyId),
        truth(el, UIA_IsExpandCollapsePatternAvailablePropertyId),
        truth(el, UIA_IsSelectionItemPatternAvailablePropertyId),
    );
    let readonly = value && truth(el, UIA_ValueIsReadOnlyPropertyId);
    let mut flags: Vec<&'static str> = Vec::new();
    for (on, flag) in [
        (invoke, "invoke"),
        (value && !readonly, "set"),
        (toggle, "toggle"),
        (expand, "expand"),
        (select, "select"),
    ] {
        if on {
            flags.push(flag);
        }
    }
    if !unsafe { el.CachedIsEnabled() }
        .map(|b| b.as_bool())
        .unwrap_or(true)
    {
        flags.push("disabled");
    }
    if cached_flag(unsafe { el.CachedIsOffscreen() }) {
        flags.push("offscreen");
    }
    if cached_flag(unsafe { el.CachedHasKeyboardFocus() }) {
        flags.push("focused");
    }
    if toggle {
        let state = number(el, UIA_ToggleToggleStatePropertyId).unwrap_or(0);
        flags.push(if state == ToggleState_On.0 {
            "checked"
        } else if state == ToggleState_Indeterminate.0 {
            "mixed"
        } else {
            "unchecked"
        });
    }
    if expand {
        let state = number(el, UIA_ExpandCollapseExpandCollapseStatePropertyId).unwrap_or(-1);
        if state == ExpandCollapseState_Expanded.0
            || state == ExpandCollapseState_PartiallyExpanded.0
        {
            flags.push("expanded");
        } else if state == ExpandCollapseState_Collapsed.0 {
            flags.push("collapsed");
        }
    }
    if select && truth(el, UIA_SelectionItemIsSelectedPropertyId) {
        flags.push("selected");
    }
    if password {
        flags.push("password");
    }
    if readonly {
        flags.push("readonly");
    }
    let at = unsafe { el.CachedBoundingRectangle() }
        .ok()
        .filter(|r| r.right > r.left && r.bottom > r.top)
        .map(|r| {
            [
                r.left - origin.0,
                r.top - origin.1,
                r.right - r.left,
                r.bottom - r.top,
            ]
        });
    Node {
        depth,
        id: runtime_id(el),
        role: role(unsafe { el.CachedControlType() }.map(|c| c.0).unwrap_or(0)),
        name: clean(
            &unsafe { el.CachedName() }
                .map(|b| b.to_string())
                .unwrap_or_default(),
            MAX_NAME,
        ),
        // What is typed into a password field is never read.
        value: (value && !password)
            .then(|| text(el, UIA_ValueValuePropertyId))
            .flatten()
            .map(|v| clean(&v, MAX_VALUE))
            .filter(|v| !v.is_empty()),
        flags,
        at,
    }
}
