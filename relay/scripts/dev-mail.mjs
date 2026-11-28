// A stand-in for the mail service in a local `wrangler dev` run: the relay posts every mail here and a test asks for
// the newest code sent to an address. Started by relay/scripts/dev-account.sh.
//   node dev-mail.mjs [port] [bind address]
//   GET /code?to=<address>  the six digits in the newest mail to that address, or 404
import http from "node:http";

const mails = new Map();
const port = Number(process.argv[2] ?? 8790);
const ip = process.argv[3] ?? "127.0.0.1";

http
  .createServer((req, res) => {
    const url = new URL(req.url, "http://sink");
    if (req.method === "POST") {
      let body = "";
      req.on("data", (c) => (body += c));
      req.on("end", () => {
        const mail = JSON.parse(body);
        mails.set(mail.to, mail);
        res.end("ok");
      });
      return;
    }
    const mail = mails.get(url.searchParams.get("to") ?? "");
    const code = /\b(\d{6})\b/.exec(mail?.text ?? "")?.[1];
    if (url.pathname !== "/code" || !code) {
      res.statusCode = 404;
      res.end("none");
      return;
    }
    mails.delete(mail.to);
    res.end(code);
  })
  .listen(port, ip);
