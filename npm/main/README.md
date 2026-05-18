# orix

High-performance package manager written in Rust, compatible with pnpm.

## Installation

```bash
npm install -g @orix/orix
# or
pnpm add -g @orix/orix
```

## Usage

```bash
orix install
orix add <package>
orix remove <package>
```

## Description

This is the JS wrapper package that selects the appropriate native binary for your platform. The actual implementation is written in Rust for maximum performance.

See the main repository for details: https://github.com/baicie/orix
