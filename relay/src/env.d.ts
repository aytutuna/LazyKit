declare namespace Cloudflare {
  interface GlobalProps {
    mainModule: typeof import("./index");
  }
  interface Env {
    ROOMS: DurableObjectNamespace<import("./room").Room>;
    GROUPS: DurableObjectNamespace<import("./group").Group>;
    PURCHASES: DurableObjectNamespace<import("./purchase").Purchase>;
  }
}
