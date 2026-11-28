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
    APPLE_APP_IDS?: string;
    APPLE_ATTEST_ENV?: string;
    PLAY_PACKAGES?: string;
    ACCOUNT_APPS?: string;
    APPLE_SIGNIN_IDS?: string;
    GOOGLE_CLIENT_IDS?: string;
    PASSWORD_STAGES?: string;
    ACCOUNT_KEY?: string;
    RESEND_API_KEY?: string;
    MAIL_FROM?: string;
  }
}
