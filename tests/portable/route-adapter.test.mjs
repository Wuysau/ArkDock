import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { contained, loadSdk, navigationInsertions, prepare } from '../../tools/preview/route-adapter.mjs';

const sdkRoot = process.env.ARKDOCK_TEST_SDK_ROOT;
const sdk = sdkRoot ? loadSdk(sdkRoot) : null;
const sdkTest = (name, fn) => test(name, { skip: sdk ? false : 'Requires installed ArkTS SDK: ARKDOCK_TEST_SDK_ROOT' }, fn);
function fixture(t) {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), 'arkdock-route-test-'));
  t.after(() => fs.rmSync(base, { recursive: true, force: true }));
  const root = path.join(base, 'original');
  const write = (file, text) => { const target = path.join(root, file); fs.mkdirSync(path.dirname(target), { recursive: true }); fs.writeFileSync(target, typeof text === 'string' ? text : JSON.stringify(text)); };
  write('build-profile.json5', { app: { signingConfigs: [{ material: { storePassword: 'must-not-copy' } }], products: [{ name: 'default', signingConfig: 'default' }] }, modules: [{ name: 'entry', srcPath: 'entry' }, { name: 'feature', srcPath: 'feature' }] });
  write('oh-package.json5', { name: 'root' });
  write('entry/oh-package.json5', { name: 'entry', dependencies: { feature_alias: 'file:../feature' } });
  write('entry/src/main/module.json5', { module: { name: 'entry', type: 'entry' } });
  write('feature/oh-package.json5', { name: 'feature_alias', dependencies: {} });
  write('feature/src/main/module.json5', { module: { name: 'feature', type: 'har', routerMap: '$profile:routes' } });
  write('feature/src/main/resources/base/profile/routes.json', { routerMap: [{ name: 'First', pageSourceFile: 'src/main/ets/First.ets', buildFunction: 'PageBuilder' }, { name: 'Second', pageSourceFile: 'src/main/ets/Second.ets', buildFunction: 'PageBuilder' }] });
  for (const name of ['First', 'Second']) write(`feature/src/main/ets/${name}.ets`, `@Builder\nexport function PageBuilder() {\n  NavDestination() { Text('${name}') }\n}\n`);
  write('entry/src/main/ets/Index.ets', '@Entry\n@Component\nstruct Index {\n  stack: NavPathStack = new NavPathStack();\n  build() { Navigation(this.stack) { Text("Home") }.hideTitleBar(true) }\n}\n');
  fs.mkdirSync(path.join(root, 'entry/oh_modules'));
  fs.symlinkSync(path.join(root, 'feature'), path.join(root, 'entry/oh_modules/feature_alias'), process.platform === 'win32' ? 'junction' : 'dir');
  const run = () => prepare({ projectRoot: root, moduleName: 'entry', outputRoot: path.join(base, 'staging'), sdkRoot }, sdk);
  return { base, root, write, run };
}

test('containment rejects sibling prefix and upward traversal', () => {
  const root = path.resolve('fixture');
  assert.equal(contained(root, path.join(root, 'child')), true);
  assert.equal(contained(root, path.resolve('fixture-other')), false);
  assert.equal(contained(root, path.resolve('fixture/../other')), false);
});

sdkTest('real SDK parser identifies Navigation but leaves comments, strings and existing dispatchers untouched', () => {
  const source = '@Component struct Example { build() {\n// Navigation() {}\nText("Navigation() {}")\nNavigation() { Text("one") }.hideTitleBar(true)\nNavigation() {}.navDestination(this.existing)\n} }';
  const plan = navigationInsertions('Example.ets', source, sdk, 'Generated');
  assert.equal(plan.edits.length, 1);
  assert.equal(plan.preserved, 1);
  assert.ok(source.slice(plan.edits[0].offset).startsWith('.hideTitleBar(true)'));
});

sdkTest('stages actual reachable builders with unique aliases without original edits', (t) => {
  const f = fixture(t);
  const original = fs.readFileSync(path.join(f.root, 'entry/src/main/ets/Index.ets'), 'utf8');
  f.write('AGENTS.md', 'must not copy');
  f.write('signing.p12', 'must not copy');
  f.write('.env.local', 'must not copy');
  f.write('.preview/stale.abc', 'must not copy');
  const report = f.run();
  assert.equal(report.routeCount, 2);
  assert.equal(report.navigationCount, 1);
  const generated = fs.readFileSync(path.join(report.root, 'entry/src/main/ets/Index.ets'), 'utf8');
  assert.match(generated, /\.navDestination\(ArkDockPreviewRoutes_/);
  assert.equal(new Set([...generated.matchAll(/PageBuilder as (\w+)/g)].map((m) => m[1])).size, 2);
  assert.match(generated, /feature_alias\/src\/main\/ets\/First/);
  assert.equal(fs.readFileSync(path.join(f.root, 'entry/src/main/ets/Index.ets'), 'utf8'), original);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(f.root, 'entry/oh-package.json5'))).dependencies, { feature_alias: 'file:../feature' });
  assert.ok(fs.realpathSync(path.join(report.root, 'entry/oh_modules/feature_alias')).startsWith(report.root));
  assert.equal(fs.existsSync(path.join(report.root, 'AGENTS.md')), false);
  assert.equal(fs.existsSync(path.join(report.root, 'signing.p12')), false);
  assert.equal(fs.existsSync(path.join(report.root, '.env.local')), false);
  assert.equal(fs.existsSync(path.join(report.root, '.preview')), false);
  assert.doesNotMatch(fs.readFileSync(path.join(report.root, 'build-profile.json5'), 'utf8'), /must-not-copy|signingConfig/);
});

sdkTest('wrapper cache is isolated and project caches are excluded', (t) => {
  const f = fixture(t);
  f.write('.hvigor/arkdock-home/wrapper/tools/node_modules/tool/index.js', 'cached tool');
  f.write('.hvigor/arkdock-home/project_caches/project.json', 'old build');
  const report = f.run();
  assert.equal(report.wrapperCache.cached, true);
  assert.equal(fs.readFileSync(path.join(report.hvigorHome, 'wrapper/tools/node_modules/tool/index.js'), 'utf8'), 'cached tool');
  assert.equal(fs.existsSync(path.join(report.hvigorHome, 'project_caches')), false);
});

sdkTest('same-module source cycles are diagnosed before generating imports', (t) => {
  const f = fixture(t);
  f.write('feature/src/main/ets/Host.ets', '@Component export struct Host { build() { Navigation() {} } }');
  f.write('feature/src/main/ets/First.ets', 'import { Host } from "./Host";\n@Builder\nexport function PageBuilder() { NavDestination() { Host() } }');
  assert.throws(f.run, /ROUTE_ADAPTER_SOURCE_IMPORT_CYCLE/);
});

sdkTest('route sources cannot escape their declared module', (t) => {
  const f = fixture(t);
  f.write('feature/src/main/resources/base/profile/routes.json', { routerMap: [{ name: 'Escape', pageSourceFile: '../entry/src/main/ets/Index.ets', buildFunction: 'PageBuilder' }] });
  assert.throws(f.run, /ROUTE_ADAPTER_SOURCE_OUTSIDE_PROJECT/);
});

sdkTest('multiple Navigation nodes receive independent AST insertions', (t) => {
  const f = fixture(t);
  f.write('entry/src/main/ets/Index.ets', '@Component struct Index { build() { Column() { Navigation() {}\nNavigation() {} } } }');
  const report = f.run();
  assert.equal(report.navigationCount, 2);
  const generated = fs.readFileSync(path.join(report.root, 'entry/src/main/ets/Index.ets'), 'utf8');
  assert.equal([...generated.matchAll(/\.navDestination\(/g)].length, 2);
});

sdkTest('absolute installed local junctions are reconstructed inside the owned snapshot', (t) => {
  const f = fixture(t);
  f.write('entry/oh-package.json5', { name: 'entry', dependencies: { renamed: 'file:../feature' } });
  fs.mkdirSync(path.join(f.root, 'entry/oh_modules'), { recursive: true });
  fs.symlinkSync(path.join(f.root, 'feature'), path.join(f.root, 'entry/oh_modules/renamed'), process.platform === 'win32' ? 'junction' : 'dir');
  const result = f.run();
  const stagedTarget = fs.realpathSync(path.join(result.root, 'entry/oh_modules/renamed'));
  assert.equal(stagedTarget, path.join(result.root, 'feature'));
  assert.equal(fs.realpathSync(path.join(f.root, 'entry/oh_modules/renamed')), path.join(f.root, 'feature'));
  assert.match(fs.readFileSync(path.join(result.root, 'entry/src/main/ets/Index.ets'), 'utf8'), /from "renamed\/src/);
});

sdkTest('new import edges that form module cycles fail before copying source', (t) => {
  const f = fixture(t);
  addHome(f);
  f.write('feature/oh-package.json5', { name: 'feature_alias', dependencies: { home: 'file:../home' } });
  assert.throws(f.run, /ROUTE_ADAPTER_IMPORT_CYCLE/);
  assert.equal(fs.existsSync(path.join(f.base, 'staging/project')), false);
});

function addHome(f) {
  const profile = JSON.parse(fs.readFileSync(path.join(f.root, 'build-profile.json5')));
  profile.modules.push({ name: 'home', srcPath: 'home' }); f.write('build-profile.json5', profile);
  f.write('entry/oh-package.json5', { name: 'entry', dependencies: { feature_alias: 'file:../feature', home: 'file:../home' } });
  f.write('home/oh-package.json5', { name: 'home', dependencies: {} });
  f.write('home/src/main/module.json5', { module: { name: 'home', type: 'har' } });
  f.write('home/src/main/ets/Home.ets', '@Component export struct Home { build() { Navigation() {} } }');
}

sdkTest('selected HAP exposes its reachable sibling route module to a host HAR only inside the snapshot', (t) => {
  const f = fixture(t); addHome(f);
  const result = f.run();
  assert.ok(result.addedDependencies.some((edge) => edge.host === 'home' && edge.target === 'feature'));
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(f.root, 'home/oh-package.json5'))).dependencies, {});
  assert.match(fs.readFileSync(path.join(result.root, 'home/src/main/ets/Home.ets'), 'utf8'), /feature_alias\/src\/main\/ets\/First/);
});

sdkTest('two independent HAPs keep separate route scopes and never create reciprocal imports', (t) => {
  const f = fixture(t);
  const profile = JSON.parse(fs.readFileSync(path.join(f.root, 'build-profile.json5')));
  profile.modules.push({ name: 'second', srcPath: 'second' }); f.write('build-profile.json5', profile);
  f.write('second/oh-package.json5', { name: 'second', dependencies: {} });
  f.write('second/src/main/module.json5', { module: { name: 'second', type: 'entry', routerMap: '$profile:routes' } });
  f.write('second/src/main/resources/base/profile/routes.json', { routerMap: [{ name: 'First', pageSourceFile: 'src/main/ets/Detail.ets', buildFunction: 'DetailBuilder' }] });
  f.write('second/src/main/ets/Index.ets', '@Entry @Component struct Index { build() { Navigation() {} } }');
  f.write('second/src/main/ets/Detail.ets', '@Builder export function DetailBuilder() { NavDestination() {} }');
  fs.mkdirSync(path.join(f.root, 'oh_modules'));
  for (const name of ['second', 'feature']) fs.symlinkSync(path.join(f.root, name), path.join(f.root, 'oh_modules', name), process.platform === 'win32' ? 'junction' : 'dir');
  const first = f.run();
  assert.equal(first.routeCount, 2); assert.equal(first.navigationCount, 1);
  assert.equal(fs.existsSync(path.join(first.root, 'second')), false);
  assert.equal(fs.existsSync(path.join(first.root, 'oh_modules/second')), false);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(first.root, 'build-profile.json5'))).modules.map((mod) => mod.name), ['entry', 'feature']);
  const second = prepare({ projectRoot: f.root, moduleName: 'second', outputRoot: path.join(f.base, 'second-output'), sdkRoot }, sdk);
  assert.equal(second.routeCount, 1); assert.equal(second.navigationCount, 1);
  assert.equal(fs.existsSync(path.join(second.root, 'entry')), false); assert.equal(fs.existsSync(path.join(second.root, 'feature')), false);
  assert.equal(fs.existsSync(path.join(second.root, 'oh_modules/feature')), false);
  assert.deepEqual(second.addedDependencies, []);
});

sdkTest('ordinary source and rawfile directories named build survive while module caches are omitted', (t) => {
  const f = fixture(t);
  f.write('entry/src/main/ets/build/Helper.ets', 'export const value: string = "source";');
  f.write('entry/src/main/resources/rawfile/build/data.json', { value: 'resource' });
  f.write('entry/build/outputs/stale.hap', 'old artifact');
  const result = f.run();
  assert.match(fs.readFileSync(path.join(result.root, 'entry/src/main/ets/build/Helper.ets'), 'utf8'), /source/);
  assert.equal(JSON.parse(fs.readFileSync(path.join(result.root, 'entry/src/main/resources/rawfile/build/data.json'))).value, 'resource');
  assert.equal(fs.existsSync(path.join(result.root, 'entry/build')), false);
});

sdkTest('duplicate route names cannot silently overwrite each other', (t) => {
  const f = fixture(t);
  f.write('feature/src/main/resources/base/profile/routes.json', { routerMap: Array(2).fill({ name: 'Duplicate', pageSourceFile: 'src/main/ets/First.ets', buildFunction: 'PageBuilder' }) });
  assert.throws(f.run, /ROUTE_ADAPTER_DUPLICATE_NAME/);
});

sdkTest('nonexported builder is a concrete unsupported route error', (t) => {
  const f = fixture(t);
  f.write('feature/src/main/ets/First.ets', '@Builder\nfunction PageBuilder() { NavDestination() {} }');
  assert.throws(f.run, /ROUTE_ADAPTER_BUILDER_EXPORT/);
});

sdkTest('preserves existing custom navDestination and does not generate a replacement snapshot', (t) => {
  const f = fixture(t);
  f.write('entry/src/main/ets/Index.ets', '@Component struct Index { build() { Navigation() {}.navDestination(this.existing) } }');
  const report = f.run();
  assert.equal(report.adapted, false);
  assert.equal(report.root, f.root);
  assert.equal(fs.existsSync(path.join(f.base, 'staging/project')), false);
});

sdkTest('projects without a system route table retain the original build root without parsing unrelated source', (t) => {
  const f = fixture(t);
  f.write('feature/src/main/module.json5', { module: { name: 'feature', type: 'har' } });
  f.write('entry/src/main/ets/Index.ets', 'Navigation syntax deliberately unknown to the standalone parser');
  const report = f.run();
  assert.equal(report.adapted, false);
  assert.equal(report.root, f.root);
  assert.equal(report.hvigorHome, null);
  assert.equal(fs.existsSync(path.join(f.base, 'staging/project')), false);
});

sdkTest('rejects output overlapping original and already existing output', (t) => {
  const f = fixture(t);
  assert.throws(() => prepare({ projectRoot: f.root, moduleName: 'entry', outputRoot: path.join(f.root, 'bad'), sdkRoot }, sdk), /OUTPUT_OVERLAP/);
  fs.mkdirSync(path.join(f.base, 'staging'));
  assert.throws(f.run, /OUTPUT_EXISTS/);
});
