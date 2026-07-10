# ImportLint

This project is going to be a linter for JavaScript and TypeScript, focused on linting import graphs.

It is going to provide the same linting functionality as [eslint-plugin-import-access](https://github.com/uhyo/eslint-plugin-import-access), but as a standalone CLI tool.

## Context

The ESLint plugin depends on type information through typescript-eslint which makes it difficult for users to migrate to oxlint. Also it is slow.

This project is going to be a standalone CLI tool that will be faster and easier to use than the ESLint plugin.

## Tech Stack

- Rust
- Uses oxc for parsing and import resolution

## Needed Features

- [ ] CLI tool
- [ ] Watch mode
- [ ] Configuration file support
- [ ] Configurable output format (including ESLint-compatible one)

## Good to have features

- [ ] VSCode extension

## Other Requirements

As a Rust-based linter, it should stay **very** fast, even for large codebases. It should be able to lint a codebase with thousands of files in a matter of seconds.

## Current Goal

To create a `docs/PLAN.md` file that outlines an actionable plan for implementing the features listed above, with no major open questions or ambiguities. The plan should be detailed enough that a developer can start implementing the features without needing to ask for clarification.

The plan should include an overall architecture of the tool, directory structure, decisions made, and a breakdown of the features into smaller tasks. It should also include any necessary research or exploration that needs to be done before implementation can begin.