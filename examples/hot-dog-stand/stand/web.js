// The counter. One page, and the page finds its neighbours by *name*.
//
// It never learns their ports and is never told their hostnames: it takes the
// host it was reached on and swaps the first label. `web.stand.localhost`
// becomes `api.stand.localhost`, and the same page works unchanged in a linked
// worktree where every name has a workspace segment in the middle. That is the
// URL fabric doing its job — an address is composed, not configured.
const http = require("node:http");

const port = Number(process.env.PORT || 0);

const PAGE = `<!doctype html>
<meta charset="utf-8">
<title>The Hot Dog Stand</title>
<style>
  body { font: 16px/1.5 ui-sans-serif, system-ui; max-width: 34rem; margin: 4rem auto; }
  button { font: inherit; padding: .4rem .8rem; margin-right: .4rem; }
  pre { background: #1113; padding: .8rem; border-radius: .4rem; overflow-x: auto; }
</style>
<h1>The Hot Dog Stand</h1>
<p id="where"></p>
<div id="menu">loading the menu…</div>
<h2>Condiments</h2>
<div id="stock">…</div>
<h2>Tickets</h2>
<pre id="log">nothing yet</pre>
<script>
  // Swap the first label of whatever host this page was reached on.
  const sibling = (label) => location.protocol + "//" +
    location.host.replace(/^[^.]+/, label);

  const api = sibling("api");
  const condiments = sibling("condiments");
  document.getElementById("where").textContent = api;

  const log = (line) => {
    const pre = document.getElementById("log");
    pre.textContent = (pre.textContent === "nothing yet" ? "" : pre.textContent + "\\n") + line;
  };

  fetch(api + "/menu").then(r => r.json()).then(({ menu }) => {
    document.getElementById("menu").innerHTML = menu.map(item =>
      '<button data-id="' + item.id + '">' + item.name + ' — ' +
      (item.price / 100).toFixed(2) + '</button>').join("");
    document.querySelectorAll("#menu button").forEach(button => {
      button.onclick = () => fetch(api + "/order?item=" + button.dataset.id)
        .then(r => r.json())
        .then(o => log("#" + o.ticket + " " + o.item.name));
    });
  }).catch(e => document.getElementById("menu").textContent = "the API is not answering: " + e);

  fetch(condiments + "/").then(r => r.json()).then(({ stock }) => {
    document.getElementById("stock").textContent = stock.join(" · ");
  }).catch(() => document.getElementById("stock").textContent = "supplier unreachable");
</script>`;

http
  .createServer((req, res) => {
    console.log(`web: ${req.method} ${req.url}`);
    res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    res.end(PAGE);
  })
  .listen(port, "127.0.0.1", () => {
    console.log(`counter open on ${port}`);
  });
