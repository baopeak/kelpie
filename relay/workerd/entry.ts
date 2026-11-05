// One script for a self-hosted relay: the router and the room together, as workerd runs them (config.capnp). On Cloudflare they are two
// scripts, so that a router deploy leaves live rooms alone.
export { default } from "../src/worker";
export { Room } from "../src/room";
