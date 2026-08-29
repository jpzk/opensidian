const inv = (c, a) => window.__TAURI__.core.invoke(c, a);
const $ = id => document.getElementById(id);
let cur = null, graphOn = false, sim = null;
let vaultPath = null, pmode = null, bpath = null;

async function refreshList() {
  const names = await inv("list_notes");
  $("notes").innerHTML = "";
  for (const n of names) {
    const li = document.createElement("li");
    li.textContent = n;
    li.className = n === cur ? "active" : "";
    li.onclick = () => open(n);
    $("notes").appendChild(li);
  }
}
async function open(name) {
  cur = name;
  showEditor();
  $("editor").value = await inv("read_note", { name });
  $("editor").focus();
  await preview();
  refreshList();
}
async function preview() {
  $("preview").innerHTML = await inv("render", { content: $("editor").value });
  for (const a of $("preview").querySelectorAll("a.wiki"))
    a.onclick = e => { e.preventDefault(); open(a.dataset.note); };
}
let t;
$("editor").oninput = () => {
  clearTimeout(t);
  t = setTimeout(async () => {
    if (cur) await inv("write_note", { name: cur, content: $("editor").value });
    preview();
  }, 250);
};
$("newbtn").onclick = async () => {
  const name = "Untitled-" + Date.now() % 10000;
  await inv("write_note", { name, content: "# " + name + "\n" });
  open(name);
};

function showEditor() {
  graphOn = false; cancelAnimationFrame(sim);
  $("graph").hidden = true;
  $("editor").style.display = ""; $("preview").style.display = "";
}
$("graphbtn").onclick = async () => {
  if (graphOn) return showEditor();
  graphOn = true;
  $("editor").style.display = "none"; $("preview").style.display = "none";
  const cv = $("graph"); cv.hidden = false;
  cv.width = cv.clientWidth; cv.height = cv.clientHeight;
  const g = await inv("graph");
  const N = g.nodes.map((n, i) => ({
    n, x: cv.width / 2 + 120 * Math.cos(i), y: cv.height / 2 + 120 * Math.sin(i),
    vx: 0, vy: 0
  }));
  const ctx = cv.getContext("2d");
  function step() {
    for (const a of N) for (const b of N) {
      if (a === b) continue;
      const dx = a.x - b.x, dy = a.y - b.y, d2 = dx * dx + dy * dy + 0.01;
      a.vx += 800 * dx / d2; a.vy += 800 * dy / d2;          // repulsion
    }
    for (const [i, j] of g.edges) {
      const a = N[i], b = N[j], dx = b.x - a.x, dy = b.y - a.y;
      a.vx += dx * 0.005; a.vy += dy * 0.005;                 // spring
      b.vx -= dx * 0.005; b.vy -= dy * 0.005;
    }
    for (const p of N) {
      p.vx += (cv.width / 2 - p.x) * 0.001; p.vy += (cv.height / 2 - p.y) * 0.001;
      p.vx *= 0.85; p.vy *= 0.85; p.x += p.vx; p.y += p.vy;
    }
    ctx.clearRect(0, 0, cv.width, cv.height);
    ctx.strokeStyle = "#45475a";
    for (const [i, j] of g.edges) {
      ctx.beginPath(); ctx.moveTo(N[i].x, N[i].y); ctx.lineTo(N[j].x, N[j].y); ctx.stroke();
    }
    ctx.fillStyle = "#89b4fa"; ctx.textAlign = "center"; ctx.font = "12px sans-serif";
    for (const p of N) {
      ctx.beginPath(); ctx.arc(p.x, p.y, 6, 0, 7); ctx.fill();
      ctx.fillText(p.n, p.x, p.y - 10);
    }
    sim = requestAnimationFrame(step);
  }
  step();
  cv.onclick = e => {
    const r = cv.getBoundingClientRect();
    const x = e.clientX - r.left, y = e.clientY - r.top;
    const hit = N.find(p => (p.x - x) ** 2 + (p.y - y) ** 2 < 144);
    if (hit) open(hit.n);
  };
};

/* ---------- vault picker ---------- */
function base(p) { return p.replace(/\/+$/, "").split("/").pop() || p; }

function showPicker() {
  pmode = null;
  $("picker").hidden = false;
  $("p-actions").style.display = "";
  $("p-sub").hidden = true;
  $("p-err").textContent = "";
  $("p-close").hidden = !vaultPath;
}
async function browseTo(p) {
  const dirs = await inv("list_dirs", { path: p });
  bpath = p;
  $("p-path").value = p;
  const ul = $("p-dirs");
  ul.innerHTML = "";
  const up = document.createElement("li");
  up.textContent = "..";
  up.onclick = () => browseTo(bpath.replace(/\/[^/]+\/?$/, "") || "/");
  ul.appendChild(up);
  for (const d of dirs) {
    const li = document.createElement("li");
    li.textContent = d + "/";
    li.onclick = () => browseTo((bpath === "/" ? "" : bpath) + "/" + d);
    ul.appendChild(li);
  }
}
async function enterMode(m) {
  pmode = m;
  $("p-actions").style.display = "none";
  $("p-sub").hidden = false;
  $("p-name").hidden = m !== "create";
  $("p-go").textContent = m === "create" ? "Create vault" : "Open this folder";
  $("p-err").textContent = "";
  await browseTo(await inv("home_dir"));
  (m === "create" ? $("p-name") : $("p-path")).focus();
}
$("p-create").onclick = () => enterMode("create");
$("p-open").onclick = () => enterMode("open");
$("p-back").onclick = showPicker;
$("p-close").onclick = () => { $("picker").hidden = true; };
$("p-path").onkeydown = e => { if (e.key === "Enter") browseTo($("p-path").value.trim()); };
$("p-name").onkeydown = e => { if (e.key === "Enter") $("p-go").click(); };
$("p-go").onclick = async () => {
  try {
    vaultPath = pmode === "create"
      ? await inv("create_vault", { parent: bpath, name: $("p-name").value })
      : await inv("set_vault", { path: bpath });
  } catch (err) { $("p-err").textContent = String(err); return; }
  $("picker").hidden = true;
  await enterVault();
};
async function enterVault() {
  $("vswitch").textContent = "⌂ " + base(vaultPath);
  cur = null;
  showEditor();
  $("editor").value = ""; $("preview").innerHTML = "";
  await refreshList();
  const first = $("notes").firstChild;
  if (first) open(first.textContent);
}
$("vswitch").onclick = showPicker;
document.addEventListener("keydown", e => {
  if (e.key === "Escape" && vaultPath && !$("picker").hidden) $("picker").hidden = true;
});

(async () => {
  vaultPath = await inv("vault_get");
  if (vaultPath) await enterVault(); else showPicker();
})();
