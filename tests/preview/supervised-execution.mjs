// Actual native execution lifetime, independent of a caller observation timeout.
process.argv[2]='execution';
await import('./supervised.mjs');
