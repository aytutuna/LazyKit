declare namespace Cloudflare {
  interface GlobalProps {
    mainModule: typeof import("./index");
  }
  interface Env {
    ROOMS: DurableObjectNamespace<import("./room").Room>;
    GROUPS: DurableObjectNamespace<import("./group").Group>;
    PURCHASES: DurableObjectNamespace<import("./purchase").Purchase>;
    QUOTAS: DurableObjectNamespace<import("./quota").Quota>;
    ACCOUNTS: DurableObjectNamespace<import("./account/account").Account>;
    IDENTITIES: DurableObjectNamespace<import("./account/identity").Identity>;
    BUDGET: DurableObjectNamespace<import("./account/account").Budget>;
    SPENT: DurableObjectNamespace<import("./account/account").Spent>;
    ADDRESSES: DurableObjectNamespace<import("./account/account").Address>;
    APPLE_APP_IDS?: string;
    APPLE_ATTEST_ENV?: string;
    PLAY_PACKAGES?: string;
    ACCOUNT_APPS?: string;
    APPLE_SIGNIN_IDS?: string;
    GOOGLE_CLIENT_IDS?: string;
    PASSWORD_STAGES?: string;
    ACCOUNT_KEY?: string;
    ID_KEY?: string;
    MAIL_DAILY_CAP?: string;
    TURNSTILE_SECRET?: string;
    RESEND_API_KEY?: string;
    MAIL_FROM?: string;
  }
}
