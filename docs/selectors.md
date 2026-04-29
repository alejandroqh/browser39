# browser39 Selector Syntax

This is the canonical reference for every selector parameter accepted by browser39 tools (`click.text`, `dom_query.selector`, `fill.selector`, `submit.selector`, etc.).

## Currently supported: CSS selectors only

Every selector parameter accepts standard CSS3 selector syntax, evaluated against the parsed HTML of the current page. There is no XPath, no jQuery-style `:contains()`, and no fuzzy text matching at the selector level (use `browser39_click` with `text` for substring link matching instead).

### Cheat sheet

| Goal | Selector |
|---|---|
| By id | `#login-form` |
| By class | `.btn-primary` |
| By tag | `form` |
| Tag + id | `form#login` |
| Tag + attribute | `input[name="user"]` |
| Attribute prefix | `a[href^="https://"]` |
| Attribute substring | `a[href*="login"]` |
| Descendant | `form input[type="submit"]` |
| Child | `ul > li` |
| First-of-type | `tr:first-of-type` |
| Nth-of-type | `tr:nth-of-type(3)` |
| Has class AND class | `button.primary.disabled` |
| Element OR element | `h1, h2, h3` |

### Stable vs fragile selectors

Prefer (stable across page updates):
- `#explicit-id`
- `[name="..."]` on form fields
- `[data-testid="..."]`
- ARIA roles: `[role="button"]`

Avoid (fragile, change on minor edits):
- Position-only: `body > div:nth-child(3) > div:nth-child(2)`
- Generated class names: `.css-1a2b3c4`
- Text-based pseudo-classes (not supported in browser39 anyway)

### Common pitfalls

- **Visible text doesn't work as a selector.** `button:contains("Submit")` is jQuery, not CSS — browser39 does not support it. To click a link by text, use `browser39_click({text: "Submit"})`. To find an element by text inside `dom_query`, use a script with `Array.from(document.querySelectorAll("button")).find(b => b.textContent.includes("Submit"))`.
- **Selectors are case-sensitive for attribute values** but case-insensitive for tag and attribute names.
- **Whitespace in attribute values** matters: `[class="foo bar"]` matches only an exact `class="foo bar"`. To match presence of one class, use the class selector: `.foo` or `.bar`.
- **Selectors run against the static parsed HTML, not the live DOM.** If a page mutates via JavaScript after load, the parsed DOM does not reflect those mutations unless you ran a script that performed them.

### Why no XPath / text= prefix

The current static HTML pipeline (`scraper` + `html5ever`) does not include an XPath engine, and adding one would mostly duplicate what CSS already covers. If you need positional or text-based queries that CSS cannot express, drop down to `browser39_dom_query` with `script` and use the JavaScript DOM API.

### Future syntax (not yet supported)

- `text="..."` prefix for text-based matching (planned)
- XPath via a `xpath:` prefix (under consideration)

These are placeholders only — do not pass them today; they will be rejected as invalid CSS.
