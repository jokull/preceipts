// The condiment supplier. Its own service, its own hostname, its own port —
// none of which it chose. $PORT arrives because the manifest gave it a `host`.
const http = require("node:http");

const port = Number(process.env.PORT || 0);
const STOCK = ["mustard", "ketchup", "remoulade", "crispy onions", "raw onions"];

http
  .createServer((req, res) => {
    const url = new URL(req.url, "http://x");
    if (url.pathname === "/health") {
      res.writeHead(200, { "content-type": "application/json" });
      return res.end(JSON.stringify({ ok: true, stock: STOCK.length }));
    }
    // CORS, because the counter page fetches this from the browser and a
    // subdomain is a different origin. This is the one line that makes
    // `web.…localhost` and `condiments.…localhost` able to talk.
    res.writeHead(200, {
      "content-type": "application/json",
      "access-control-allow-origin": "*",
    });
    console.log(`condiments: served ${url.pathname}`);
    res.end(JSON.stringify({ stock: STOCK }));
  })
  .listen(port, "127.0.0.1", () => {
    console.log(`condiments ready on ${port}`);
  });
