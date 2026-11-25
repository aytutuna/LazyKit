declare namespace Cloudflare {
  interface GlobalProps {
    mainModule: typeof import("./index");
  }
  interface Env {
    ROOMS: DurableObjectNamespace<import("./room").Room>;
  }
}
