// The room script's module (wrangler.room.toml): its Durable Object classes and the handler a script must have, nothing else. The
// runtime takes every export of a script's main module for an entrypoint and refuses one that is a plain value, such as the constants
// room.ts shares with the tests.
export { default, Room } from "./room";
export { Registrar } from "./registrar";
