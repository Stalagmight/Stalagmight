# Stalagmight user guide

How to use Stalagmight. Pages link to each other.

**Workflows**
- [Creating a new survey](workflows/creating-a-new-survey.md)
- [Setting a zero datum](workflows/setting-a-zero-datum.md)
- [Fixing warnings](workflows/fixing-warnings.md)
- [Importing from an MNemo](workflows/importing-from-mnemo.md)
- [Importing an Ariane survey](workflows/importing-from-ariane.md)
- [Merging stations](workflows/merging-stations.md)
- [Closing a loop](workflows/closing-a-loop.md)
- [Adding comments](workflows/adding-comments.md)

**Screens**
- [Start screen](screens/start-screen.md)
- [New Project dialog](screens/new-project-dialog.md)
- [Survey view](screens/survey-view.md)
- [Import / Create Section dialog](screens/create-section-dialog.md)
- [Placement mode](screens/placement-mode.md)
- [Station context menu](screens/station-context-menu.md)
- [Edit Station dialog](screens/edit-station-dialog.md)
- [Warnings panel](screens/warnings-panel.md)
- [DMP import](screens/dmp-import.md)
- [Create Project for Import dialog](screens/create-project-for-import.md)
- [Import Ariane Survey dialog](screens/ariane-import.md)
- [Import report](screens/import-report.md)
- [Merge Stations dialog](screens/merge-stations-dialog.md)
- [Create Loop Closure dialog](screens/closure-dialog.md)
- [Display menu](screens/display-menu.md)
- [Data view](screens/data-view.md)

**Concepts**
- [Sections, shots and stations](concepts/sections-shots-stations.md)
- [Errors and warnings](concepts/errors-and-warnings.md)

## Contributing

- Keep pages short. Most important first, details at the end.
- Link to a page instead of repeating it.
- `workflows/` = tasks, `screens/` = one page per screen, `concepts/` = terms
  and ideas. New kind of page = new folder, listed above.
- A new page goes in the lists above and in its folder's `index.md`, in the
  same position.
- Screenshots: `images/<what-it-shows>.png`, 1920x1080 window, clean profile (no
  recent projects, signed out of SpeleoDB). Use example data made for the docs,
  never a private survey or account details.
- Number every interactive element with a callout placed beside it, never on
  top of it. Screen pages list the elements in callout order; other pages refer
  to them as **Name** (n).

## Publishing on the website

Every page here except this README is published at
<https://stalagmight.com/docs/> within six hours of reaching `master`.

- Start each page with front matter:

  ```yaml
  ---
  title: Importing from an MNemo
  description: One sentence for search results and link cards, 120-160 characters.
  order: 4
  ---
  ```

  Without `title` the `# Heading` is used; without `description`, the first
  paragraph or the first steps. `order` sets the position in the sidebar and in
  previous / next; pages without it sort alphabetically. Follow the order of
  the lists above.
- A folder's `index.md` is that section's page, and its `order` places the
  section in the sidebar. `index.md` at the top of this folder is the landing
  page, `/docs/`.
- Optional keys: `tags` (pages sharing a tag list each other under Related
  topics), `related` (paths, listed first), `since: 0.6.0` (shows "Coming in
  0.6.0" until that version is released), `aliases` (old URLs after a rename,
  e.g. `[/docs/workflows/old-name/]`), `draft: true` (keeps a page off the
  site), `feature` (links the page from a home-page feature: `mnemo`,
  `editing`, `speleodb`, `planning`, `import-export`, `open-source`).
- Links and anchors are checked on every website build. A broken one shows as
  plain text with a wavy underline and a warning in the website's build log; it
  does not stop publishing.
- `> [!NOTE]`, `TIP`, `IMPORTANT`, `WARNING` and `CAUTION` render as callouts
  on the site as on GitHub.
- Keep committing PNG screenshots. The site serves them as WebP sized for the
  page and opens the full image on click.
- Preview with the [website repo](https://github.com/Stalagmight/StagWeb):

  ```bash
  git clone https://github.com/Stalagmight/StagWeb && cd StagWeb && npm ci
  DOCS_SOURCE=/path/to/Stalagmight/docs/user-guide npx vite --host localhost
  ```

  Then open <http://localhost:8084/docs/>; pages rebuild on save. Search needs
  a full build: `DOCS_SOURCE=... npm run build && npx vite preview --host localhost`.
