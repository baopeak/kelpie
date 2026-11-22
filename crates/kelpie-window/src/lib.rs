//! Kelpie's window helper: an agent sees and operates the windows of an app it started, the way a person would use the app, while the
//! person keeps the mouse, the keyboard and the window in front.
//!
//! It reads a window through UI Automation (the accessibility tree: roles, names, values, states) and acts through the same interface
//! (press, set a value, toggle, expand, select), so nothing moves the cursor or takes the focus unless `focus` is asked for. A picture of
//! the window comes from the window itself (`PrintWindow`), so it works while the window is covered.
//!
//! **Reach.** An agent reaches only the windows of programs it started with `launch`, and of their child processes (a web view's
//! processes are children of its app). The person's own windows, Kelpie's included, are never read and never operated: what is on them
//! is not the agent's to see, and a program that can press buttons in Kelpie's window could approve its own requests. This is a rule of
//! this tool, not a boundary of the system: a program running as the same user can do all of this without it.
//!
//! What decides anything is in the modules below, which need no Windows and are tested as such; the native code (`win`) reads the machine
//! and carries the decisions out:
//! - `tree`: the outline an agent reads, which nodes are kept, node ids, and the actions.
//! - `scope`: which programs were started by the agent, and whether a window's process descends from one.

pub mod scope;
pub mod tree;

#[cfg(all(windows, feature = "helper"))]
pub mod win;
