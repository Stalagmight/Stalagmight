# Writing the user guide

This folder is the Stalagmight user guide. Every Markdown file in it is
published at **<https://stalagmight.com/docs/>**, except this README and pages
marked as drafts. A change merged to `master` is on the site within six hours;
the website rebuilds on a schedule and reads this folder each time.

Write for someone using the app, not for someone working on its code. The rest
of `docs/` is developer documentation and stays off the site.

## Where a file ends up

| File here | Page on the site |
|---|---|
| `import/mnemo-dmp.md` | `/docs/import/mnemo-dmp/` |
| `import/index.md` | `/docs/import/`, the section's own page |
| a folder without `index.md` | `/docs/<folder>/`, generated: a list of its pages |
| `index.md` | `/docs/`, the landing page, followed by a list of every section |
| `README.md` | not published |

Each top-level folder is a section in the sidebar. URLs are the file names in
lowercase, with anything other than letters and digits turned into hyphens, so
keep names short, lowercase and hyphenated: `gas-planning.md`, not
`Gas Planning.md`.

## Front matter

Start each page with a block like this. Every key is optional, but `title`
and `description` are worth always setting.

```markdown
---
title: Importing mNemo DMP files
description: Bring raw mNemo survey data into Stalagmight and place each section.
order: 1
tags: [import, mnemo]
related: [../concepts/zero-datum.md]
---
```

| Key | What it does |
|---|---|
| `title` | The page title. Without it, the page's leading `# Heading` is used, then the file name. |
| `description` | One sentence, ideally 120-160 characters. It is the snippet in search results (Google's and the site's own) and on link cards. Without it the first paragraph is used, cut at 160 characters. |
| `order` | Position within the folder (lower comes first). Pages without one follow, alphabetically. On a section's `index.md` it orders the sections. |
| `tags` | Pages that share tags list each other under **Related topics**. |
| `related` | Relative paths to other pages, listed first under **Related topics**. |
| `feature` | Links this page from a feature on the home page: `open-source`, `mnemo`, `editing`, `speleodb`, `planning` or `import-export`. Give each key to one page. |
| `since` | The version that introduces what the page describes, e.g. `0.6.0`. While the current release is older, the page shows a "Coming in 0.6.0" notice. |
| `aliases` | Old URLs that should keep working after a page is renamed or moved, e.g. `[/docs/import/ariane-tml/]`. |
| `draft` | `true` keeps the page off the site. Links to a draft are reported as broken. |

## Linking pages together

Link with ordinary relative Markdown links. They work when the files are read
on GitHub too, and the site turns them into links between its pages:

```markdown
See [placing sections](placing-sections.md) and the
[suggest step](placing-sections.md#the-suggest-step).
Shots are explained under [stations and shots](../concepts/stations-and-shots.md#shots).
The whole import section: [importing data](../import/).
```

- **Anchors** follow GitHub's rules: lowercase, spaces become hyphens,
  punctuation is dropped, and a repeated heading gets `-1`, `-2`. An anchor
  that works on GitHub works on the site.
- **Every link is checked** when the site builds. A link to a missing page,
  heading, image or draft shows on the site as plain text with a wavy
  underline, and is reported in the website's build log. It does not stop the
  site from publishing.
- **Links to the rest of the repository** (`../SPEC.md`) go to that file on
  GitHub.
- **Link generously.** Each page lists the pages that link to it under
  **Linked from**, so links are what tie the guide together.

## Images and other files

Keep them in this folder (an `images/` folder per section works well) and use
Markdown syntax:

```markdown
![The import wizard listing the sections of a DMP file](images/import-wizard.png)
```

- Write alt text that says what the image shows; it is what screen readers
  and search engines get.
- Use PNG or WebP screenshots of the app window only, cropped to the part that
  matters, at most about 1600 px wide.
- Raw HTML `<img>` tags are not checked or copied. Use the Markdown form.
- Other files, such as a sample survey to try an import with, can be linked the
  same way; the site publishes a copy for download.

## Callouts

GitHub's alert syntax renders as a callout on the site as well as on GitHub:

```markdown
> [!WARNING]
> Check the device's compass calibration before trusting a survey.
```

The kinds are `NOTE`, `TIP`, `IMPORTANT`, `WARNING` and `CAUTION`. Keep
`CAUTION` for things that matter to a diver's safety.

## Writing so people find it

- **One topic per page.** A page that answers one question well ranks better,
  and is easier to link to, than a long page covering several.
- **Put the words people search for in the title and the first paragraph.**
  Say "importing mNemo DMP files", not "getting your data in".
- **Start with what the reader can do, then how.** The first paragraph is often
  the snippet people see in search results and AI answers.
- **State facts plainly.** "Stalagmight imports mNemo `.dmp` files, versions 2
  to 5" is quotable; "lots of formats are supported" is not.
- **Name things as the app does**, with **bold** for buttons and menu items:
  **Import**, **Plan**, **Export**.

## Repository rules that apply here

- **ASCII hyphens only.** The `lint:no-em-dash` check covers this folder: no en
  or em dashes, no Unicode minus. Write `0-360`, and use a plain hyphen or a
  comma in prose.
- **No internal references.** `lint:private-leaks` covers this folder too.
- Commits follow the usual style: `docs: ...`.

## Previewing

GitHub renders these files closely enough for most edits. To see the real site,
with the sidebar, links and search, build the
[website](https://github.com/Stalagmight/StagWeb) against your checkout:

```bash
git clone https://github.com/Stalagmight/StagWeb && cd StagWeb && npm ci
DOCS_SOURCE=/path/to/Stalagmight/docs/user-guide npx vite --host localhost
```

Then open <http://localhost:8084/docs/>. The dev server rebuilds the pages when
you save, and prints a warning for every broken link. Search needs a full
build:

```bash
DOCS_SOURCE=/path/to/Stalagmight/docs/user-guide npm run build
npx vite preview --host localhost
```
