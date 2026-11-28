import { MailFailedError, MailNotConfiguredError, type Mailer } from "./core/email";

export function resendMailer(env: { RESEND_API_KEY?: string; MAIL_FROM?: string }): Mailer {
  return {
    async send(mail) {
      if (!env.RESEND_API_KEY || !env.MAIL_FROM) throw new MailNotConfiguredError("email sending is not configured");
      const res = await fetch("https://api.resend.com/emails", {
        method: "POST",
        headers: { Authorization: `Bearer ${env.RESEND_API_KEY}`, "Content-Type": "application/json" },
        body: JSON.stringify({ from: env.MAIL_FROM, to: [mail.to], subject: mail.subject, text: mail.text }),
      });
      if (!res.ok) throw new MailFailedError(`mail service answered ${res.status}`);
    },
  };
}
