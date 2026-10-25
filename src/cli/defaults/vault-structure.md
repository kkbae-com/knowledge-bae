# Vault structure

This vault holds engineering notes for one or more software projects. The vault
sits next to the folder that contains the code repositories, never inside it.

## Layout

```
<vault>/
  README.md                  index of the projects and how the vault is organized
  <Project>/
    README.md                what the project is; lists the repositories it spans
    Architecture.md          how the pieces fit together
    Local Startup.md         how to run the project locally
    stories/
      <ticketid-slug>/
        <ticketid-slug>.md   the story note
        PRs.md               pull requests the story needs, grouped by repository
        investigations/      investigations tied to this story
        bugs/                bugs found while working this story
    investigations/          investigations with no story yet
    bugs/                    bugs not tied to a story
```

## Rules

- One folder per project. A project can span several repositories; list them in
  the project's README so a repository can be mapped back to its project.
- A story is always a folder named `ticketid-slug`, for example
  `36257-merge-duplicate-skills`. Its main note has the same name as the folder.
- `PRs.md` lists the pull requests a story needs, grouped by repository, with
  their status. Use it even when a story touches a single repository.
- Bugs are named `id-slug`, for example `92-duplicate-ledger-bug`.
- Investigations are named by slug only, for example `skill-name-dedup-options`.
- An investigation starts in the project's `investigations/` folder. If it
  leads to a story, move it into that story's `investigations/` folder when the
  story is opened, keep its name, and link to it from the story note. Further
  investigation for a story is created in the story's folder.
- Note names are the titles Obsidian shows and links resolve by, so keep them
  descriptive and unique.
