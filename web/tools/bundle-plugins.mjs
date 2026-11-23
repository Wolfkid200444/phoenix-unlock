// Bundles plugins/*/src/index.ts into web/plugins/<name>.js the way scripts/phoenix.nu does (esbuild IIFE,
// es2023, the leading `// @` lines kept as a banner) and lists them in web/plugins/index.json for the site.
// usage: node web/tools/bundle-plugins.mjs   (esbuild on PATH, or ESBUILD=path/to/esbuild)
// The bundles are not committed: run this before web/tools/dev-server.mjs or node-build.mjs; CI runs it for the site.
// It also writes web/version.json, what the site ships, for the runtime's update check (crates/runtime/src/update.rs):
// the loader's baked version and each plugin bundle's CRC-32.
// The native artifacts next to them are copied from a loader build (target/aarch64-linux-android/release):
//   libphoenix.so -> libphoenix-arm64.so, build/phoenix-hook/*/out/libshadowhook_nothing.so -> libshadowhook_nothing-arm64.so
import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { crc32 } from 'node:zlib';

const root = fileURLToPath(new URL('../../', import.meta.url));
const out = fileURLToPath(new URL('../plugins/', import.meta.url));
const esbuild = process.env.ESBUILD ?? 'esbuild';
const index = [];
for (const name of readdirSync(`${root}plugins`).sort()) {
  const entry = `${root}plugins/${name}/src/index.ts`;
  if (!existsSync(entry)) continue;
  const lines = readFileSync(entry, 'utf8').split(/\r?\n/);
  const header = lines.slice(0, Math.max(0, lines.findIndex((l) => !l.startsWith('// @')))).join('\n');
  const r = spawnSync(esbuild, [entry, '--bundle', '--format=iife', '--target=es2023', '--log-level=warning', `--banner:js=${header}`, `--outfile=${out}${name}.js`],
    { stdio: 'inherit' });
  if (r.status !== 0) process.exit(r.status ?? 1);
  const warning = header.match(/^\/\/ @warning (.*)$/m)?.[1] ?? '';
  index.push({
    name,
    description: header.match(/^\/\/ @description (.*)$/m)?.[1] ?? '',
    ...(warning ? { warning } : {}),
  });
}
writeFileSync(`${out}index.json`, JSON.stringify(index, null, 2) + '\n');
// crates/runtime/src/update.rs marks the version as `phoenix-version:<version>;`. An older loader has none: `0`
// is older than every build, so nobody is told to update to it.
const loader = readFileSync(`${out}libphoenix-arm64.so`, 'latin1').match(/phoenix-version:([\w.-]+);/)?.[1] ?? '0';
const plugins = Object.fromEntries(index.map(({ name }) => [name, crc32(readFileSync(`${out}${name}.js`))]));
writeFileSync(fileURLToPath(new URL('../version.json', import.meta.url)), JSON.stringify({ loader, plugins }, null, 2) + '\n');
console.log(`bundled ${index.map((p) => p.name).join(', ')}`);
