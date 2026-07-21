# Pam

Pam is an experimental, opt-in Pi package that recreates Amp's low / medium /
high / ultra capability dial using models routed through Alex. It is kept
under `plugins/` and is not installed or enabled by Alex.

## Try it

First refresh Pi's Alex model catalog, then load Pam for one run:

```sh
alex connect pi
pi -e ./plugins/pam
```

To install the local experiment explicitly:

```sh
pi install ./plugins/pam
```

Loading Pam starts at `medium`, matching Amp's recommendation. Press `Ctrl+S`
or run `/pam` to open the dial. Use left/right, Enter to apply, or Escape to
cancel. Press `S` from the dial to open model settings; closing settings returns
to the same place on the dial.
Modes can also be selected directly:

```text
/pam low
/pam medium
/pam high
/pam ultra
/pam settings
/pam off
```

`/pam off` stops Pam's prompt and status handling but deliberately leaves the
currently selected Pi model unchanged. Selecting a model or reasoning effort
with Pi's normal controls also disables the active Pam mode.

## Settings

Run `/pam settings` to configure the agent and oracle model for every tier in
the TUI:

1. Use Up/Down to choose a tier and its `agent` or `oracle` slot.
2. Press Enter to open the model picker.
3. Type to search the `alex/*` models currently available to Pi.
4. Use Up/Down and Enter to save the selected model.

The selection is written to [`settings.json`](./settings.json) immediately. If
the changed slot is the active tier's agent, Pam also switches the running Pi
session to the new model immediately.

You can still edit `settings.json` by hand. Models must use an `alex/*` ID from
Pi's Alex catalog; run `/reload` in Pi after manual edits. Pam resolves
those routing IDs through Pi's model catalog and displays their familiar names,
such as `GPT-5.6 Luna`, while retaining the exact `alex/*` ID for requests.

Each choice has a primary `model`, optional `fallbacks`, and a reasoning
`thinking` level:

```json
{
  "medium": {
    "agent": {
      "model": "alex/gpt-5.6-sol",
      "thinking": "medium"
    },
    "oracle": {
      "model": "alex/claude-fable-5",
      "thinking": "high"
    }
  }
}
```

The checked-in file contains all four levels. Missing or invalid fields fall
back to Pam's defaults and produce a warning instead of preventing the plugin
from loading.

## Wiring

The checked-in defaults are:

| Mode | Agent | Effort | Oracle | Effort |
|---|---|---:|---|---:|
| low | `alex/openrouter/z-ai/glm-5.2` | medium | `alex/gpt-5.6-sol` | high |
| medium | `alex/gpt-5.6-sol` | medium | `alex/gpt-5.6-sol` | high |
| high | `alex/gpt-5.6-sol` | xhigh | `alex/claude-fable-5` | high |
| ultra | `alex/claude-fable-5` | high | `alex/gpt-5.6-sol` | high |

The direct Sol and Fable routes are preferred. Their Alex OpenRouter
routes are accepted as fallbacks when those are the models present in Pi's
catalog. Low intentionally has no silent fallback: if GLM-5.2 is unavailable,
Pam reports that the Alex catalog needs refreshing.

Pam registers `pam_oracle`, an LLM-callable tool that sends a concrete review
question to the active mode's oracle model. The oracle call uses its own model
and reasoning effort without changing the main agent.

This package changes no Alex configuration and contains no install hook.
Remove an explicit local installation with:

```sh
pi remove ./plugins/pam
```
