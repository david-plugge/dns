# Dashboard components

Button, Card, Input and Badge are source-owned copies from [Dioxus Components](https://github.com/DioxusLabs/components/tree/9a758255ea26e2b20c8cecf4c1c946feb9e71da7), imported with `dx components add`. The accompanying `assets/dx-components-theme.css` comes from the same registry. These files use the upstream [MIT license](LICENSE-MIT).

Unused button/badge variants and the verified-icon example are omitted. Card and Badge merge custom classes with their CSS-module classes instead of replacing them. Component styles remain local CSS modules; application layout and dark-theme overrides live in `assets/main.css`. The primitives dependency is pinned to the matching registry revision.

Keep local changes when comparing or importing future registry updates; `dx components add --force` overwrites the owned source.
