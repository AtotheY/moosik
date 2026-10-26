# Moosik UI work

Before building UI, inspect Home, Library, and a detail page in the running app. Read `crates/refryt/src/theme.rs`, shared `widgets.rs`, and nearby `ui` implementations. Consult relevant visual references first.

Use the existing Inter weights, semantic palette, page/section type sizes, 20pt page inset, 56pt rows, 40pt row artwork, pill filters, Lucide icons, themed menus, and artwork shapes. Prefer shared widgets over stock egui controls. Keep detail in drilldowns, tooltips, or dialogs; do not crowd the primary view with explanatory copy.

Validate the running result visually at full and narrow window sizes, with empty, sparse, and populated content. Check truncation, alignment, spacing, focus/hover states, and reachability of actions. Do not declare completion based only on tests/build success.
