import { captureEvent, preserveCampaignParameters } from "./analytics.js";

document.documentElement.classList.add("js-ready");

const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;
const routeInterest = { provider: null, harness: null };

function recordRouteInterest() {
  const status = document.querySelector("[data-route-interest]");
  if (!routeInterest.provider || !routeInterest.harness) {
    if (status) status.textContent = "Pick a provider and harness to see the route you are interested in.";
    return;
  }
  if (status) status.textContent = `${routeInterest.provider} → Alex → ${routeInterest.harness}`;
  captureEvent("route_interest_selected", routeInterest);
}

function setupDemo(demo) {
  const steps = [...demo.querySelectorAll("[data-demo-step]")];
  const startButton = demo.querySelector("[data-action='start']");
  const stepButton = demo.querySelector("[data-action='step']");
  const resetButton = demo.querySelector("[data-action='reset']");
  const progress = demo.querySelector("[data-demo-progress]");
  const liveStatus = demo.querySelector("[data-demo-status]");
  const ruleDetails = demo.querySelector("[data-rule]");
  const demoId = demo.dataset.demo;

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
    demo.style.setProperty("--demo-progress", `${percentage}%`);

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

  render();
}

document.querySelectorAll("[data-demo]").forEach(setupDemo);

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
    routeInterest.provider = button.dataset.provider;
    recordRouteInterest();
  });
}

for (const button of document.querySelectorAll("[data-harness]")) {
  button.addEventListener("click", () => {
    document.querySelectorAll("[data-harness]").forEach((item) => {
      item.setAttribute("aria-pressed", String(item === button));
    });
    routeInterest.harness = button.dataset.harness;
    recordRouteInterest();
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

for (const link of document.querySelectorAll("[data-demo-action]")) {
  link.addEventListener("click", () => {
    captureEvent("demo_action_clicked", {
      demo_id: link.closest("[data-demo]")?.dataset.demo,
      action: link.dataset.demoAction
    });
  });
}
