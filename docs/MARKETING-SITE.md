# Marketing site

The CommonKit marketing site is a dependency-free static page in this
directory:

- `index.html` contains the page structure and copy.
- `site.css` contains the responsive visual system.
- `case-study-unsold-group.md` contains the long-form case study.

GitHub Pages can publish the site directly from the repository's `/docs`
folder on the default branch. This repository does not use GitHub Actions, so
Pages must use branch-based publishing.

## Publish

1. Merge the site files into the default branch.
2. Open the repository's **Settings → Pages**.
3. Select **Deploy from a branch**.
4. Select the default branch and the `/docs` folder.
5. Save the configuration.
6. Open `https://unsoldgroup.github.io/commonkit/` and verify the page on a
   narrow and wide viewport.

GitHub performs the hosting operation after the branch changes. GitHub-hosted
workflow status is not a CommonKit validation or release gate.

## Content rules

- Lead with portability across agents, machines, projects, and colleagues.
- Keep “100x” attached to the Unsold.Group case study and describe it as a
  direction for cumulative leverage, not a benchmark.
- Use observed counts only when their inventory date is clear.
- Keep unfinished capabilities and target boundaries explicit.
- Never place secret values in the site, its source, or its build artifacts.
