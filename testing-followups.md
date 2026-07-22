# Bugs found while writing parser/translator tests (testing/core-parser-tests)

Found by table-driven tests against recorded fixtures; tests assert CURRENT
behavior where noted, so fixing these will require updating those tests.

1. `gemini_to_anthropic` reads only `parameters`; recorded Gemini requests use
   `parametersJsonSchema`, so detailed tool schemas collapse to `{"type":"object"}`.
2. Gemini tool-call IDs are not preserved — replaced with synthetic
   `toolu_gemini_*`; Anthropic→Gemini response translation omits the ID.
3. Gemini tool results are joined by function NAME not call ID — ambiguous for
   repeated calls to the same function.
4. OpenAI Responses `tool_choice` ignored in both directions.
5. Anthropic→Gemini emits no `toolConfig`, so `tool_choice` doesn't round-trip.
6. OpenAI Chat→Anthropic `tool_choice` drops `none` and `required`.
7. Anthropic→Responses drops `temperature`, `top_p`, stop sequences.
