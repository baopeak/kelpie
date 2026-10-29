// What was sent from the reply box, for the up arrow: this page's memory only (a terminal may have asked for a
// password, and nothing typed on the phone outlives the pairing).
import { Composer } from "../../src/lib/composer-core";

export const history = new Composer();

/** Forgetting or losing the pairing empties it too: after pairing again in the same tab the up arrow must not recall the earlier text. */
export function forgetHistory() {
  history.clear();
}
