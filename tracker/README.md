# Local Markdown Tracker

This project has no external issue tracker, so wayfinding lives here as markdown files.

## Wayfinding operations

- **Map**: [map.md](map.md) — frontmatter label `wayfinder:map`. The canonical low-res view.
- **Tickets**: `tickets/NN-slug.md` — children of the map via `parent: map` frontmatter. Refer to tickets by their **title**, never by bare id.
- **Labels**: `wayfinder:research | prototype | grilling | task` in the `labels` frontmatter list.
- **Claim**: set `assignee:` in the ticket frontmatter **before any work**. Open + unassigned = unclaimed.
- **Blocking**: `blocked-by: [<ids>]` frontmatter. A ticket is unblocked when every listed id has `status: closed`.
- **Frontier** (open, unblocked, unclaimed):

  ```bash
  cd tracker/tickets
  for f in *.md; do
    st=$(grep -m1 '^status:' "$f" | awk '{print $2}')
    as=$(grep -m1 '^assignee:' "$f" | sed 's/assignee: *//')
    [ "$st" = open ] && [ -z "$as" ] || continue
    blocked=no
    for dep in $(grep -m1 '^blocked-by:' "$f" | tr -d 'blocked-by:[],'); do
      grep -l "^id: $dep$" *.md | xargs grep -m1 '^status:' | grep -q closed || blocked=yes
    done
    [ "$blocked" = no ] && echo "FRONTIER: $f"
  done
  ```

- **Resolution**: append a `## Resolution` section to the ticket, set `status: closed`, and add a one-line pointer to the map's *Decisions so far*.
- **Assets**: files in `assets/`, linked from tickets — never pasted into ticket bodies.
