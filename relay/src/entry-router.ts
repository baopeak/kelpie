// The router script's module (wrangler.toml): only its handler. The runtime takes every export of a script's main module for an
// entrypoint and refuses one that is a plain value, such as the constants worker.ts shares with the tests.
export { default } from "./worker";
