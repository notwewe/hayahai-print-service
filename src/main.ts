import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";

interface Printer {
  queueId: string;
  displayName: string;
  driverName?: string;
  available: boolean;
}

interface Status {
  paired: boolean;
  apiUrl?: string;
  agentId?: string;
  printers: Printer[];
  activeJob: boolean;
  lastError?: string;
  version: string;
}

const app = document.querySelector<HTMLElement>("#app")!;
let pairingUri = "";

function escape(value: string) {
  const node = document.createElement("span");
  node.textContent = value;
  return node.innerHTML;
}

async function render() {
  const status = await invoke<Status>("status");
  app.innerHTML = `
    <section class="card">
      <div class="brand"><span>H</span><div><h1>HayahAI Print Service</h1><p>Version ${escape(status.version)}</p></div></div>
      <div class="state ${status.paired ? "online" : "waiting"}">
        ${status.paired ? (status.activeJob ? "Printing" : "Connected") : "Waiting for pairing"}
      </div>
      ${status.paired && !pairingUri ? `
        <dl><dt>Client API</dt><dd>${escape(status.apiUrl ?? "")}</dd><dt>Agent</dt><dd>${escape(status.agentId ?? "")}</dd></dl>
        <h2>Installed printer queues</h2>
        <ul>${status.printers.length ? status.printers.map((printer) => `<li><strong>${escape(printer.displayName)}</strong><br><small>${escape(printer.driverName ?? "OS driver")} · ${printer.available ? "available" : "offline"}</small></li>`).join("") : "<li>Waiting for the next printer scan…</li>"}</ul>
      ` : `
        <p>${status.paired ? "Confirm the new pairing link to replace this workstation’s current connection." : "Generate a pairing link in TMS under Preferences → Printing Presets, then open it on this workstation."}</p>
        <form id="pair-form"><label>Pairing link<input id="pair-uri" required value="${escape(pairingUri)}" placeholder="hayahai-print://pair?..." /></label><button type="submit">Pair workstation</button></form>
      `}
      ${status.lastError ? `<p class="error">${escape(status.lastError)}</p>` : ""}
      <p class="foot">This service uses outbound HTTPS and installed operating-system printer queues. Receipt content is never written to its logs.</p>
    </section>`;
  document.querySelector("#pair-form")?.addEventListener("submit", async (event) => {
    event.preventDefault();
    const button = document.querySelector<HTMLButtonElement>("button")!;
    const input = document.querySelector<HTMLInputElement>("#pair-uri")!;
    button.disabled = true;
    try {
      await invoke("pair", { pairingUri: input.value.trim() });
      pairingUri = "";
      await render();
    } catch (error) {
      app.insertAdjacentHTML("beforeend", `<p class="error">${escape(String(error))}</p>`);
      button.disabled = false;
    }
  });
}

await listen<string>("pair-request", async (event) => {
  pairingUri = event.payload;
  await render();
  const input = document.querySelector<HTMLInputElement>("#pair-uri");
  if (input) input.value = event.payload;
});

await render();
window.setInterval(() => void render(), 5_000);
