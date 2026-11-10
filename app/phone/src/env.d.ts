declare module "virtual:phone-vi" {
  const dictionary: Record<string, string>;
  export default dictionary;
}

/** Set by the build (vite.phone.config.ts): true in the app served from the relay's origin, false in the daemon's. Absent in tests. */
declare const __HOSTED__: boolean;
/** The app's version, set by the build. Absent in tests. */
declare const __APP_VERSION__: string;
