// Lighting the coals. A one-shot: it prints, it exits 0, and everything with
// `needs = ["grill"]` was waiting on exactly that.
const started = Date.now();
const stages = ["stacking the coals", "lighting", "waiting for grey ash"];

(async () => {
  for (const stage of stages) {
    console.log(`grill: ${stage}`);
    await new Promise((r) => setTimeout(r, 120));
  }
  console.log(`grill: hot, ${Date.now() - started}ms`);
})();
