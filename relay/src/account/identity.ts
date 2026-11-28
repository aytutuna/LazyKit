import { DurableObject } from "cloudflare:workers";
import { durableStore } from "./account";
import { IdentityCore } from "./core/identity";

export class Identity extends DurableObject<Cloudflare.Env> {
  private core: IdentityCore;

  constructor(ctx: DurableObjectState, env: Cloudflare.Env) {
    super(ctx, env);
    this.core = new IdentityCore(durableStore(ctx));
    ctx.blockConcurrencyWhile(() => this.core.load());
  }

  account() {
    return this.core.account();
  }

  bind(...a: Parameters<IdentityCore["bind"]>) {
    return this.core.bind(...a);
  }

  claim(...a: Parameters<IdentityCore["claim"]>) {
    return this.core.claim(...a);
  }

  release(...a: Parameters<IdentityCore["release"]>) {
    return this.core.release(...a);
  }

  use(...a: Parameters<IdentityCore["use"]>) {
    return this.core.use(...a);
  }

  erase() {
    return this.core.erase();
  }

}
