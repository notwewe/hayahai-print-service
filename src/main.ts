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
let showPairingForm = false;
let pairingError = "";

function escape(value: string) {
  const node = document.createElement("span");
  node.textContent = value;
  return node.innerHTML;
}

async function render() {
  const status = await invoke<Status>("status");
  const pairing = !status.paired || showPairingForm || Boolean(pairingUri);
  app.innerHTML = `
    <section class="card">
      <div class="brand"><span>H</span><div><h1>HayahAI Print Service</h1><p>Version ${escape(status.version)}</p></div></div>
      <div class="state ${status.paired ? "online" : "waiting"}">
        ${status.paired ? (status.activeJob ? "Printing" : "Connected") : "Waiting for pairing"}
      </div>
      ${status.paired && !pairing ? `
        <dl><dt>Client API</dt><dd>${escape(status.apiUrl ?? "")}</dd><dt>Agent</dt><dd>${escape(status.agentId ?? "")}</dd></dl>
        <h2>Installed printer queues</h2>
        <ul>${status.printers.length ? status.printers.map((printer) => `<li><strong>${escape(printer.displayName)}</strong><br><small>${escape(printer.driverName ?? "OS driver")} · ${printer.available ? "available" : "offline"}</small></li>`).join("") : "<li>Waiting for the next printer scan…</li>"}</ul>
        <button id="new-pairing" class="secondary" type="button">Pair a new link</button>
      ` : `
        <p>${status.paired ? "Paste a new pairing link to replace this workstation’s current connection. The current connection remains saved unless the new pairing succeeds." : "Generate a pairing link in TMS under Preferences → Printing Presets, then open it on this workstation."}</p>
        <form id="pair-form">
          <label>New pairing link<input id="pair-uri" required value="${escape(pairingUri)}" placeholder="hayahai-print://pair?..." autocomplete="off" spellcheck="false" /></label>
          <div class="actions">
            <button type="submit">${status.paired ? "Replace pairing" : "Pair workstation"}</button>
            ${status.paired ? '<button id="cancel-pairing" class="secondary" type="button">Cancel</button>' : ""}
          </div>
        </form>
      `}
      ${pairingError ? `<p class="error">${escape(pairingError)}</p>` : ""}
      ${status.lastError ? `<p class="error">${escape(status.lastError)}</p>` : ""}
      <p class="foot">This service uses outbound HTTPS and installed operating-system printer queues. Receipt content is never written to its logs.</p>
    </section>`;
  document.querySelector("#pair-form")?.addEventListener("submit", async (event) => {
    event.preventDefault();
    const button = document.querySelector<HTMLButtonElement>("#pair-form button[type=submit]")!;
    const input = document.querySelector<HTMLInputElement>("#pair-uri")!;
    button.disabled = true;
    pairingError = "";
    try {
      await invoke("pair", { pairingUri: input.value.trim() });
      pairingUri = "";
      showPairingForm = false;
      await render();
    } catch (error) {
      pairingError = String(error);
      await render();
    }
  });
  document.querySelector<HTMLInputElement>("#pair-uri")?.addEventListener("input", (event) => {
    pairingUri = (event.currentTarget as HTMLInputElement).value;
  });
  document.querySelector("#new-pairing")?.addEventListener("click", async () => {
    pairingError = "";
    showPairingForm = true;
    await render();
    document.querySelector<HTMLInputElement>("#pair-uri")?.focus();
  });
  document.querySelector("#cancel-pairing")?.addEventListener("click", async () => {
    pairingUri = "";
    pairingError = "";
    showPairingForm = false;
    await render();
  });
}

await listen<string>("pair-request", async (event) => {
  pairingUri = event.payload;
  pairingError = "";
  showPairingForm = true;
  await render();
  const input = document.querySelector<HTMLInputElement>("#pair-uri");
  if (input) input.value = event.payload;
});

await render();
window.setInterval(() => {
  if (!document.querySelector("#pair-uri")) void render();
}, 5_000);
