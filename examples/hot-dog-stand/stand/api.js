// Orders. Deliberately chatty: a log line per request is what makes the
// `output` instrument in the lab panel worth looking at.
const http = require("node:http");

const port = Number(process.env.PORT || 0);
const MENU = [
  { id: "plain", name: "Plain dog", price: 550 },
  { id: "works", name: "One with everything", price: 790 },
  { id: "double", name: "Double, no judgement", price: 1090 },
];

let orders = 0;

http
  .createServer((req, res) => {
    const url = new URL(req.url, "http://x");
    const json = (code, body) => {
      res.writeHead(code, {
        "content-type": "application/json",
        "access-control-allow-origin": "*",
      });
      res.end(JSON.stringify(body));
    };

    if (url.pathname === "/health") return json(200, { ok: true, orders });
    if (url.pathname === "/menu") {
      console.log("api: GET /menu");
      return json(200, { menu: MENU });
    }
    if (url.pathname === "/order") {
      const id = url.searchParams.get("item") || "plain";
      const item = MENU.find((m) => m.id === id);
      if (!item) {
        console.log(`api: 404 unknown item ${id}`);
        return json(404, { error: `no such dog: ${id}` });
      }
      orders += 1;
      console.log(`api: order #${orders} — ${item.name} — sssssss`);
      return json(200, { ticket: orders, item });
    }
    console.log(`api: 404 ${url.pathname}`);
    json(404, { error: "not on the menu" });
  })
  .listen(port, "127.0.0.1", () => {
    console.log(`api: listening on ${port}`);
  });
