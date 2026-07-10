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
