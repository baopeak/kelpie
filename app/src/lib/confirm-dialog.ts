/** Asks an OK / Cancel question in a native dialog. tauri-plugin-dialog replaces `window.confirm`
 *  with an async version, so a bare `if (!confirm(...))` sees a truthy Promise and goes ahead
 *  before the user answers. Always await this instead. (Plain browsers and e2e stubs return a
 *  boolean, which resolves the same way.) */
export const confirmDialog = (message: string): Promise<boolean> => Promise.resolve(window.confirm(message));
