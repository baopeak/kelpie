// A block a program writes once can reach the terminal twice or more, some 0.5 s apart: seen on
// tmux's start over Windows' ssh.exe under ConPTY. Answering each copy is what hurts: tmux takes
// the first device attributes reply and passes any later one on to the shell, which prints what
// it cannot parse (`1;2c0;276;0c` after the prompt).
//
// Only a secondary query opens the window, as ConPTY's own start asks the primary one alone: a
// program asking right after that still gets its answer. Two programs starting within the window
// lose little: the second gets no reply, and tmux goes on without it after its own timeout.

const REPEAT_MS = 1500;

export function deviceQueryGate(now: () => number = Date.now) {
  let secondaryAt = -Infinity;
  const inWindow = () => now() - secondaryAt < REPEAT_MS;
  return {
    /** Whether this primary query (`CSI c`) repeats one already answered. */
    primaryRepeats: inWindow,
    /** Whether this secondary query (`CSI > c`) repeats one already answered. */
    secondaryRepeats(): boolean {
      if (inWindow()) return true;
      secondaryAt = now();
      return false;
    },
  };
}
