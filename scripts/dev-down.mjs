// Same identity, context checks and command lock as startup; no volume deletion.
process.argv.push('--down');
await import('./dev.mjs');
