# Debugging Orix

Orix separates user-facing progress output from developer diagnostics.

## User progress

The install progress UI is rendered by the CLI reporter and uses stderr.

## Debug log file

Enable debug logs:

```bash
orix install --debug
```

Specify a log file:

```bash
orix install --debug --log-file ./orix-debug.log
```

Use environment variables:

```bash
ORIX_DEBUG=1 ORIX_LOG=orix=trace,orix_resolver=debug orix install
```

## Console tracing

Console tracing disables the live progress UI to avoid stderr conflicts:

```bash
ORIX_LOG=orix=debug orix install --no-progress
```

## Useful filters

```bash
ORIX_LOG=orix=debug
ORIX_LOG=orix=trace,orix_fetcher=debug
ORIX_LOG=orix_core=debug,orix_resolver=trace
```

## Privacy

Orix must not log authentication tokens. Logs may include:

- registry URL
- whether authentication is configured
- package names and versions
- store/cache paths
- phase durations
- error chains

Logs must not include:

- npm tokens
- auth headers
- full authorization values
