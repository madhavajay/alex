import { captureEvent, preserveCampaignParameters } from "./analytics.js";

document.documentElement.classList.add("js-ready");

const demo = document.querySelector("[data-demo]");
const steps = [...document.querySelectorAll("[data-demo-step]")];
const startButton = document.querySelector("[data-action='start']");
const stepButton = document.querySelector("[data-action='step']");
const resetButton = document.querySelector("[data-action='reset']");
const progress = document.querySelector("[data-demo-progress]");
const liveStatus = document.querySelector("[data-demo-status]");
const ruleDetails = document.querySelector("[data-rule]");
const demoId = demo?.dataset.demo ?? "fable-to-sol-overload";
const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;

let currentStep = -1;
let timer = null;
let started = false;
let completed = false;

function announce(message) {
  if (liveStatus) liveStatus.textContent = message;
}

function recordStart(entryPoint) {
  if (started) return;
  started = true;
  captureEvent("demo_started", { demo_id: demoId, entry_point: entryPoint });
}

function stop() {
  if (timer) window.clearTimeout(timer);
  timer = null;
  if (startButton && currentStep < steps.length - 1) {
    startButton.textContent = "Play";
    startButton.setAttribute("aria-pressed", "false");
  }
}

function render() {
  steps.forEach((item, index) => {
    item.classList.toggle("is-complete", index < currentStep);
    item.classList.toggle("is-active", index === currentStep);
    if (index === currentStep) item.setAttribute("aria-current", "step");
    else item.removeAttribute("aria-current");
  });

  const shown = Math.max(0, currentStep + 1);
  const percentage = steps.length ? Math.round((shown / steps.length) * 100) : 0;
  if (progress) {
    progress.value = shown;
    progress.max = steps.length;
    progress.setAttribute("aria-valuetext", `${shown} of ${steps.length} routing steps`);
  }
  document.documentElement.style.setProperty("--demo-progress", `${percentage}%`);

  if (currentStep >= 0 && steps[currentStep]) {
    announce(`Step ${shown} of ${steps.length}: ${steps[currentStep].dataset.label}`);
  } else {
    announce("Demo ready. Play it or move one step at a time.");
  }

  if (currentStep === steps.length - 1 && !completed) {
    completed = true;
    stop();
    startButton.textContent = "Completed";
    startButton.disabled = true;
    captureEvent("demo_completed", { demo_id: demoId, steps_count: steps.length });
  }
}

function advance(entryPoint = "step_control") {
  recordStart(entryPoint);
  if (currentStep < steps.length - 1) currentStep += 1;
  render();
}

function scheduleAdvance() {
  if (!timer) return;
  advance("play_control");
  if (currentStep < steps.length - 1) {
    timer = window.setTimeout(scheduleAdvance, reducedMotion ? 1500 : 1200);
  }
}

startButton?.addEventListener("click", () => {
  if (timer) {
    stop();
    announce(`Paused after step ${Math.max(0, currentStep + 1)}.`);
    return;
  }
  recordStart("play_control");
  startButton.textContent = "Pause";
  startButton.setAttribute("aria-pressed", "true");
  timer = window.setTimeout(scheduleAdvance, 20);
});

stepButton?.addEventListener("click", () => {
  stop();
  advance("step_control");
});

resetButton?.addEventListener("click", () => {
  stop();
  currentStep = -1;
  started = false;
  completed = false;
  startButton.disabled = false;
  startButton.textContent = "Play";
  render();
});

ruleDetails?.addEventListener("toggle", () => {
  if (ruleDetails.open) {
    captureEvent("rule_revealed", {
      demo_id: demoId,
      rule_id: ruleDetails.dataset.rule
    });
  }
});

for (const link of document.querySelectorAll("[data-campaign-link]")) {
  preserveCampaignParameters(link);
}

for (const button of document.querySelectorAll("[data-provider]")) {
  button.addEventListener("click", () => {
    document.querySelectorAll("[data-provider]").forEach((item) => {
      item.setAttribute("aria-pressed", String(item === button));
    });
    captureEvent("provider_selected", {
      provider: button.dataset.provider,
      surface: "provider_interest"
    });
  });
}

document.querySelector("[data-install-copy]")?.addEventListener("click", async (event) => {
  const button = event.currentTarget;
  const command = button.dataset.installCopy;
  try {
    await navigator.clipboard.writeText(command);
    button.textContent = "Copied";
  } catch {
    const range = document.createRange();
    range.selectNode(document.querySelector("[data-install-command]"));
    getSelection().removeAllRanges();
    getSelection().addRange(range);
    button.textContent = "Selected — press copy";
  }
  captureEvent("install_copied", { surface: "demo_completion" });
});

document.querySelector("[data-download]")?.addEventListener("click", (event) => {
  captureEvent("download_clicked", {
    platform: event.currentTarget.dataset.download,
    surface: "demo_completion"
  });
});

document.querySelector("[data-cliproxyapi-docs]")?.addEventListener("click", () => {
  captureEvent("cliproxyapi_docs_opened", { surface: "provider_interest" });
});

render();
