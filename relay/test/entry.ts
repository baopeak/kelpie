// In production the router and the room are two scripts. Tests run them as one, because the test
// helpers only reach Durable Objects defined in the main Worker.
export { default } from "../src/worker";
export { Room } from "../src/room";
