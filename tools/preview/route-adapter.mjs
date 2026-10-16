// Preview-only source adaptation. This program never writes into projectRoot or the SDK.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const MAX_CONFIG = 1024 * 1024;
const MAX_SOURCE = 4 * 1024 * 1024;
const LIMITS = { files: 50000, bytes: 768 * 1024 * 1024, routes: 1024, modules: 256 };
const CACHE_DIR = new Set(['.hvigor', '.preview', 'build', 'node_modules', '.arkdock-preview']);
const OMIT_FILE = /^(?:AGENTS|CLAUDE|SKILL)\.md$|^\.env(?:\.|$)|^\.(?:npmrc|ohpmrc)$|(?:^|[._-])(?:credentials?|secrets?|private[-_]?key)(?:[._-]|$)|\.(?:p12|pfx|pem|key|keystore|jks|p7b|cer|crt)$/i;
const digest = (value) => crypto.createHash('sha256').update(value).digest('hex');
const fail = (code, detail = '') => { throw new Error(`${code}${detail ? `: ${detail}` : ''}`); };
const slash = (value) => value.split(path.sep).join('/');
export function contained(root, candidate) {
  const relative = path.relative(root, candidate);
  return relative === '' || (!relative.startsWith(`..${path.sep}`) && relative !== '..' && !path.isAbsolute(relative));
}
function boundedText(file, limit = MAX_CONFIG) {
  const stat = fs.statSync(file);
  if (!stat.isFile() || stat.size > limit) fail('ROUTE_ADAPTER_FILE_LIMIT', path.basename(file));
  const text = fs.readFileSync(file, 'utf8');
  if (Buffer.byteLength(text) > limit) fail('ROUTE_ADAPTER_FILE_LIMIT');
  return text;
}
export function loadSdk(sdkRoot) {
  const loader = path.join(sdkRoot, 'sdk/default/openharmony/ets/build-tools/ets-loader');
  const ts = require(path.join(loader, 'node_modules/typescript'));
  const json5 = require(path.join(loader, 'node_modules/json5'));
  const options = json5.parse(boundedText(path.join(loader, 'tsconfig.json'))).compilerOptions;
  if (typeof ts.isEtsComponentExpression !== 'function' || !options?.ets?.components) fail('ROUTE_ADAPTER_SDK_PARSER_UNSUPPORTED');
  return { ts, json5, options };
}
function parseSource(file, text, sdk) {
  const sf = sdk.ts.createSourceFile(file, text, sdk.ts.ScriptTarget.Latest, true, sdk.ts.ScriptKind.ETS, sdk.options);
  if (sf.parseDiagnostics.length) fail('ROUTE_ADAPTER_ARKTS_PARSE', `${path.basename(file)}:${sf.parseDiagnostics[0].start}`);
  return sf;
}
function readConfig(file, sdk) { return sdk.json5.parse(boundedText(file)); }
function canonicalInside(root, file) {
  const actual = fs.realpathSync(file);
  if (!contained(root, actual)) fail('ROUTE_ADAPTER_SOURCE_OUTSIDE_PROJECT');
  return actual;
}
function moduleInfo(root, item, sdk) {
  if (typeof item.name !== 'string' || typeof item.srcPath !== 'string') fail('ROUTE_ADAPTER_MODULE_INVALID');
  const dir = canonicalInside(root, path.resolve(root, item.srcPath));
  const manifestFile = path.join(dir, 'oh-package.json5');
  const manifest = fs.existsSync(manifestFile) ? readConfig(manifestFile, sdk) : {};
  const config = readConfig(path.join(dir, 'src/main/module.json5'), sdk);
  return { name: item.name, dir, relative: path.relative(root, dir), manifest, packageName: manifest.name, config };
}
function reachableModules(root, definitions, selectedName, sdk) {
  const selected = definitions.find((item) => item.name === selectedName);
  if (!selected) fail('ROUTE_ADAPTER_MODULE_MISSING');
  const declarations = definitions.map((item) => {
    if (typeof item.name !== 'string' || typeof item.srcPath !== 'string') fail('ROUTE_ADAPTER_MODULE_INVALID');
    const declared = path.resolve(root, item.srcPath);
    // Unrelated product modules may not even be checked out. Their manifests,
    // routes and sources must not gate the selected HAP's preview build.
    return { item, declared, actual: fs.existsSync(declared) ? fs.realpathSync(declared) : null };
  });
  const result = [], visited = new Set(), queue = [selected];
  while (queue.length) {
    const definition = queue.shift();
    if (visited.has(definition.name)) continue;
    visited.add(definition.name);
    const mod = moduleInfo(root, definition, sdk);
    result.push(mod);
    const dependencies = { ...mod.manifest.dependencies, ...mod.manifest.dynamicDependencies };
    for (const [alias, spec] of Object.entries(dependencies)) {
      let dependency;
      if (typeof spec === 'string' && spec.startsWith('file:')) dependency = path.resolve(mod.dir, spec.slice(5));
      else {
        // Registry packages can also be installed as local module junctions.
        // Resolve the actual installed package; never match a bare package name
        // to an unrelated project module with coincidentally the same name.
        for (const directory of [mod.dir, root]) {
          const installed = path.join(directory, 'oh_modules', alias);
          if (fs.existsSync(installed)) { dependency = installed; break; }
        }
      }
      if (!dependency || !fs.existsSync(dependency)) continue;
      const actual = fs.realpathSync(dependency);
      const declaration = declarations.find((item) => item.actual === actual);
      if (declaration) queue.push(declaration.item);
    }
  }
  const excluded = declarations.filter(({ item }) => !visited.has(item.name)).map(({ declared }) => declared)
    .filter((directory) => !result.some((mod) => contained(directory, mod.dir)));
  return { modules: result, excluded };
}
function routeList(root, modules, sdk) {
  const routes = [];
  const names = new Set();
  for (const mod of modules) {
    const profile = mod.config.module?.routerMap;
    if (!profile) continue;
    if (typeof profile !== 'string' || !/^\$profile:[\w-]+$/.test(profile)) fail('ROUTE_ADAPTER_PROFILE_INVALID');
    const file = canonicalInside(root, path.join(mod.dir, 'src/main/resources/base/profile', `${profile.slice(9)}.json`));
    const list = readConfig(file, sdk).routerMap;
    if (!Array.isArray(list)) fail('ROUTE_ADAPTER_ROUTES_INVALID');
    for (const route of list) {
      if (typeof route.name !== 'string' || !route.name || route.name.length > 256 || [...route.name].some((ch) => ch.charCodeAt(0) < 32) ||
          typeof route.pageSourceFile !== 'string' || !/\.ets$/.test(route.pageSourceFile) ||
          !/^[A-Za-z_$][\w$]*$/.test(route.buildFunction)) fail('ROUTE_ADAPTER_ROUTE_INVALID');
      if (names.has(route.name)) fail('ROUTE_ADAPTER_DUPLICATE_NAME', route.name);
      names.add(route.name);
      const source = canonicalInside(mod.dir, path.resolve(mod.dir, route.pageSourceFile));
      const text = boundedText(source, MAX_SOURCE);
      const sf = parseSource(source, text, sdk);
      const fn = sf.statements.find((node) => sdk.ts.isFunctionDeclaration(node) && node.name?.text === route.buildFunction);
      const modifiers = fn?.modifiers ?? [];
      const exported = modifiers.some((node) => node.kind === sdk.ts.SyntaxKind.ExportKeyword);
      const builder = [...modifiers, ...(fn?.illegalDecorators ?? [])].some((node) => node.kind === sdk.ts.SyntaxKind.Decorator && node.expression.getText(sf) === 'Builder');
      if (!fn || !exported || !builder) fail('ROUTE_ADAPTER_BUILDER_EXPORT', route.name);
      const parameters = fn.parameters;
      if (parameters.length > 2 || parameters.some((p) => p.dotDotDotToken || !sdk.ts.isIdentifier(p.name)) ||
          (parameters.length >= 1 && parameters[0].type?.getText(sf) !== 'string') ||
          (parameters.length === 2 && !['Object', 'object'].includes(parameters[1].type?.getText(sf)))) {
        fail('ROUTE_ADAPTER_BUILDER_SIGNATURE', route.name);
      }
      routes.push({ name: route.name, buildFunction: route.buildFunction, source, module: mod, arity: parameters.length, sourceHash: digest(text) });
      if (routes.length > LIMITS.routes) fail('ROUTE_ADAPTER_ROUTES_LIMIT');
    }
  }
  return routes.sort((a, b) => a.name.localeCompare(b.name, 'en'));
}

// Byte ranges come from the SDK's ArkTS AST, not a regular-expression source rewrite.
export function navigationInsertions(file, text, sdk, dispatcher) {
  const sf = parseSource(file, text, sdk);
  const edits = [];
  let preserved = 0;
  function visit(node) {
    if (sdk.ts.isEtsComponentExpression(node) && node.expression.getText(sf) === 'Navigation') {
      let chain = node;
      let hasDestination = false;
      while (sdk.ts.isPropertyAccessExpression(chain.parent) && chain.parent.expression === chain &&
             sdk.ts.isCallExpression(chain.parent.parent) && chain.parent.parent.expression === chain.parent) {
        if (chain.parent.name.text === 'navDestination') hasDestination = true;
        chain = chain.parent.parent;
      }
      if (hasDestination) preserved++;
      else edits.push({ offset: node.end, text: `.navDestination(${dispatcher})` });
    }
    sdk.ts.forEachChild(node, visit);
  }
  visit(sf);
  return { edits, preserved };
}
function sourceFiles(mod) {
  const result = [];
  let entries = 0;
  function walk(dir, depth = 0) {
    if (depth > 64) fail('ROUTE_ADAPTER_DIRECTORY_DEPTH');
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      if (++entries > LIMITS.files) fail('ROUTE_ADAPTER_FILES_LIMIT');
      const file = path.join(dir, entry.name);
      if (entry.isSymbolicLink()) fail('ROUTE_ADAPTER_SOURCE_LINK');
      if (entry.isDirectory()) walk(file, depth + 1);
      else if (entry.isFile() && entry.name.endsWith('.ets')) result.push(file);
      if (result.length > LIMITS.files) fail('ROUTE_ADAPTER_FILES_LIMIT');
    }
  }
  const dir = path.join(mod.dir, 'src/main/ets');
  if (fs.existsSync(dir)) walk(dir);
  return result;
}
function dependencyGraph(modules) {
  const graph = new Map(modules.map((mod) => [mod.dir, new Set()]));
  for (const mod of modules) {
    for (const spec of Object.values(mod.manifest.dependencies ?? {})) {
      if (typeof spec !== 'string' || !spec.startsWith('file:')) continue;
      const target = fs.realpathSync(path.resolve(mod.dir, spec.slice(5)));
      if (graph.has(target)) graph.get(mod.dir).add(target);
    }
  }
  return graph;
}
function reaches(graph, from, target, visited = new Set()) {
  if (from === target) return true;
  if (visited.has(from)) return false;
  visited.add(from);
  return [...(graph.get(from) ?? [])].some((next) => reaches(graph, next, target, visited));
}
function aliasFor(host, target, graph, added) {
  if (host.dir === target.dir) return null;
  const dependencies = host.manifest.dependencies ??= {};
  for (const [alias, spec] of Object.entries(dependencies)) {
    if (typeof spec === 'string' && spec.startsWith('file:') && fs.realpathSync(path.resolve(host.dir, spec.slice(5))) === target.dir) return alias;
  }
  const alias = target.packageName;
  if (typeof alias !== 'string' || !/^(?:@[\w.-]+\/)?[\w.-]+$/.test(alias) || dependencies[alias]) fail('ROUTE_ADAPTER_PACKAGE_ALIAS');
  if (reaches(graph, target.dir, host.dir)) fail('ROUTE_ADAPTER_IMPORT_CYCLE', `${host.name} -> ${target.name}`);
  dependencies[alias] = `file:${slash(path.relative(host.dir, target.dir))}`;
  graph.get(host.dir).add(target.dir);
  added.push({ host: host.name, target: target.name, alias });
  return alias;
}
function checkSourceCycles(root, hosts, routes, sdk) {
  const edges = new Map();
  function resolveImport(file, specifier) {
    let candidate;
    if (specifier.startsWith('.')) candidate = path.resolve(path.dirname(file), specifier);
    else {
      const parts = specifier.split('/');
      const packageName = specifier.startsWith('@') ? parts.slice(0, 2).join('/') : parts[0];
      const suffix = parts.slice(specifier.startsWith('@') ? 2 : 1).join('/');
      let dir = path.dirname(file);
      while (contained(root, dir)) {
        const installed = path.join(dir, 'oh_modules', packageName);
        if (fs.existsSync(path.join(installed, 'oh-package.json5'))) {
          const pkg = readConfig(path.join(installed, 'oh-package.json5'), sdk);
          candidate = path.join(installed, suffix || pkg.main || 'Index.ets');
          break;
        }
        const parent = path.dirname(dir);
        if (parent === dir) break;
        dir = parent;
      }
    }
    if (!candidate) return null; // System/native imports cannot import a source host.
    for (const trial of [candidate, `${candidate}.ets`, `${candidate}.ts`, path.join(candidate, 'Index.ets')]) {
      if (!fs.existsSync(trial) || !fs.statSync(trial).isFile() || !/\.(ets|ts)$/.test(trial)) continue;
      const actual = fs.realpathSync(trial);
      if (contained(root, actual)) return actual;
    }
    return null;
  }
  function dependencies(file) {
    if (edges.has(file)) return edges.get(file);
    if (edges.size >= LIMITS.files) fail('ROUTE_ADAPTER_IMPORT_GRAPH_LIMIT');
    const found = new Set();
    edges.set(file, found);
    const sf = parseSource(file, boundedText(file, MAX_SOURCE), sdk);
    for (const statement of sf.statements) {
      if ((!sdk.ts.isImportDeclaration(statement) && !sdk.ts.isExportDeclaration(statement)) || !statement.moduleSpecifier) continue;
      const resolved = resolveImport(file, statement.moduleSpecifier.text);
      if (resolved) found.add(resolved);
    }
    return found;
  }
  function importsHost(file, host, seen = new Set()) {
    if (file === host) return true;
    if (seen.has(file)) return false;
    seen.add(file);
    return [...dependencies(file)].some((dependency) => importsHost(dependency, host, seen));
  }
  for (const host of hosts) {
    for (const route of routes) {
      if (host.file === route.source) continue; // Call the existing local builder directly.
      if (importsHost(route.source, host.file)) fail('ROUTE_ADAPTER_SOURCE_IMPORT_CYCLE', route.name);
      dependencies(host.file).add(route.source);
    }
  }
}
function snapshot(root, destination, stats, modules, excluded) {
  const links = [];
  const cacheRoots = new Set([root, ...modules.map((mod) => mod.dir)]);
  let entries = 0;
  function copy(source, target, depth = 0) {
    if (++entries > LIMITS.files || depth > 64) fail('ROUTE_ADAPTER_SNAPSHOT_LIMIT');
    const stat = fs.lstatSync(source);
    if (stat.isSymbolicLink()) { links.push({ source, target }); return; }
    if (stat.isDirectory()) {
      fs.mkdirSync(target, { recursive: true });
      for (const item of fs.readdirSync(source)) {
        const child = path.join(source, item);
        if (item === '.git' || item === '.idea' || cacheRoots.has(source) && CACHE_DIR.has(item) || OMIT_FILE.test(item)
            || excluded.some((directory) => path.relative(directory, child) === '')) continue;
        if (source === root && item === 'build-profile.json5') continue;
        copy(child, path.join(target, item), depth + 1);
      }
    } else if (stat.isFile()) {
      stats.files++;
      stats.bytes += stat.size;
      if (stats.files > LIMITS.files || stats.bytes > LIMITS.bytes || stat.size > 128 * 1024 * 1024) fail('ROUTE_ADAPTER_SNAPSHOT_LIMIT');
      fs.copyFileSync(source, target, fs.constants.COPYFILE_EXCL);
    } else fail('ROUTE_ADAPTER_FILE_TYPE');
  }
  copy(root, destination);
  const external = new Map();
  for (let index = 0; index < links.length; index++) {
    if (index > LIMITS.files) fail('ROUTE_ADAPTER_LINKS_LIMIT');
    const { source, target } = links[index];
    const actual = fs.realpathSync(source);
    // A workspace-level OHPM store can retain links for another HAP. Those
    // installed but unreachable modules are deliberately absent from this run.
    if (source.includes(`${path.sep}oh_modules${path.sep}`) && excluded.some((directory) => contained(directory, actual))) continue;
    let staged;
    if (contained(root, actual)) staged = path.join(destination, path.relative(root, actual));
    else {
      staged = external.get(actual);
      if (!staged) {
        if (!source.includes(`${path.sep}oh_modules${path.sep}`)) fail('ROUTE_ADAPTER_EXTERNAL_LINK');
        if (!fs.existsSync(path.join(actual, 'oh-package.json5'))) fail('ROUTE_ADAPTER_EXTERNAL_PACKAGE');
        staged = path.join(destination, '.arkdock-packages', digest(actual).slice(0, 20));
        external.set(actual, staged);
        copy(actual, staged);
      }
    }
    // Never retain a link into the original workspace or an excluded original cache.
    if (!fs.existsSync(staged) || !contained(destination, staged)) fail('ROUTE_ADAPTER_LINK_TARGET');
    if (!fs.statSync(staged).isDirectory()) fail('ROUTE_ADAPTER_FILE_LINK_UNSUPPORTED');
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.symlinkSync(staged, target, process.platform === 'win32' ? 'junction' : 'dir');
  }
}
function writeJson(file, value) {
  const text = JSON.stringify(value, null, 2) + '\n';
  if (Buffer.byteLength(text) > 4 * MAX_CONFIG) fail('ROUTE_ADAPTER_REPORT_LIMIT');
  fs.writeFileSync(file, text, { flag: 'wx' });
}
function snapshotWrapper(root, output) {
  const home = path.join(output, 'hvigor-home');
  fs.mkdirSync(home);
  const source = path.join(root, '.hvigor/arkdock-home/wrapper');
  const stats = { files: 0, bytes: 0, cached: false };
  if (!fs.existsSync(source)) return { home, ...stats };
  if (!contained(root, fs.realpathSync(source))) fail('ROUTE_ADAPTER_WRAPPER_OUTSIDE_PROJECT');
  function copy(from, to) {
    const stat = fs.lstatSync(from);
    if (stat.isSymbolicLink()) fail('ROUTE_ADAPTER_WRAPPER_LINK_UNSUPPORTED');
    if (stat.isDirectory()) {
      fs.mkdirSync(to, { recursive: true });
      for (const entry of fs.readdirSync(from)) copy(path.join(from, entry), path.join(to, entry));
    } else if (stat.isFile()) {
      if (++stats.files > 15000 || (stats.bytes += stat.size) > 256 * 1024 * 1024) fail('ROUTE_ADAPTER_WRAPPER_LIMIT');
      fs.copyFileSync(from, to, fs.constants.COPYFILE_EXCL);
    } else fail('ROUTE_ADAPTER_WRAPPER_FILE_TYPE');
  }
  // The wrapper itself may contain node_modules: this is an installed build
  // tool cache, isolated separately from both source and task metadata.
  copy(source, path.join(home, 'wrapper'));
  stats.cached = true;
  return { home, ...stats };
}
export function prepare({ projectRoot, moduleName, outputRoot, sdkRoot, reportPath }, suppliedSdk) {
  const root = fs.realpathSync(projectRoot);
  const output = path.resolve(outputRoot);
  if (contained(root, output) || contained(output, root)) fail('ROUTE_ADAPTER_OUTPUT_OVERLAP');
  if (fs.existsSync(output)) fail('ROUTE_ADAPTER_OUTPUT_EXISTS');
  // Parent is owned by the caller. Resolve it first so a reparse point cannot
  // turn a seemingly external destination into a write inside the original.
  const parent = fs.realpathSync(path.dirname(output));
  const actualOutput = path.join(parent, path.basename(output));
  if (contained(root, actualOutput) || contained(actualOutput, root)) fail('ROUTE_ADAPTER_OUTPUT_OVERLAP');
  const report = path.resolve(reportPath ?? path.join(output, 'report.json'));
  if (!contained(output, report)) fail('ROUTE_ADAPTER_REPORT_OUTSIDE_OUTPUT');
  const sdk = suppliedSdk ?? loadSdk(sdkRoot);
  const profile = readConfig(path.join(root, 'build-profile.json5'), sdk);
  if (!Array.isArray(profile.modules) || profile.modules.length > LIMITS.modules) fail('ROUTE_ADAPTER_MODULES_LIMIT');
  const { modules, excluded } = reachableModules(root, profile.modules, moduleName, sdk);
  const selected = modules.find((mod) => mod.name === moduleName);
  if (!selected) fail('ROUTE_ADAPTER_MODULE_MISSING');
  const routes = routeList(root, modules, sdk);
  // Ordinary page-only projects need no source scan, AST transformation, or
  // snapshot. In particular, unrelated ArkTS syntax must not block this path.
  if (!routes.length) {
    fs.mkdirSync(output, { recursive: true });
    const result = { version: 1, adapted: false, root, module: selected.dir, routeCount: 0, reportPath: report, sourceMapPath: null, hvigorHome: null };
    writeJson(report, result);
    return result;
  }
  const hosts = [];
  const sources = [];
  for (const mod of modules) {
    for (const file of sourceFiles(mod)) {
      const text = boundedText(file, MAX_SOURCE);
      if (!text.includes('Navigation')) continue;
      const token = digest(path.relative(root, file)).slice(0, 12);
      const dispatcher = `ArkDockPreviewRoutes_${token}`;
      if (text.includes(dispatcher)) fail('ROUTE_ADAPTER_SYMBOL_COLLISION');
      const plan = navigationInsertions(file, text, sdk, dispatcher);
      if (plan.edits.length && routes.length) hosts.push({ mod, file, text, dispatcher, edits: plan.edits });
      if (plan.preserved) sources.push({ source: file, preservedDispatchers: plan.preserved });
    }
  }
  fs.mkdirSync(output, { recursive: true });
  if (!hosts.length) {
    const result = { version: 1, adapted: false, root, module: selected.dir, routeCount: 0, reportPath: report, sourceMapPath: null, hvigorHome: null };
    writeJson(report, result);
    return result;
  }
  const destination = path.join(output, 'project');
  const graph = dependencyGraph(modules);
  const added = [];
  checkSourceCycles(root, hosts, routes, sdk);
  for (const host of hosts) {
    host.imports = routes.map((route) => {
      const alias = aliasFor(host.mod, route.module, graph, added);
      const file = slash(path.relative(route.module.dir, route.source)).replace(/\.ets$/, '');
      return alias ? `${alias}/${file}` : './' + slash(path.relative(path.dirname(host.file), route.source)).replace(/\.ets$/, '');
    });
  }
  const stats = { files: 0, bytes: 0 };
  snapshot(root, destination, stats, modules, excluded);
  for (const route of routes) {
    if (digest(boundedText(path.join(destination, path.relative(root, route.source)), MAX_SOURCE)) !== route.sourceHash) fail('ROUTE_ADAPTER_SOURCE_CHANGED');
  }
  for (const host of hosts) {
    if (digest(boundedText(path.join(destination, path.relative(root, host.file)), MAX_SOURCE)) !== digest(host.text)) fail('ROUTE_ADAPTER_SOURCE_CHANGED');
  }
  const wrapper = snapshotWrapper(root, output);
  // PreviewBuild does not sign. Keep certificate references and passwords out of the snapshot.
  if (profile.app) {
    delete profile.app.signingConfigs;
    for (const product of profile.app.products ?? []) delete product.signingConfig;
  }
  // Hvigor's module graph must describe the same scoped source snapshot; do not
  // sync unrelated HAPs or add their runtime routes to this build.
  const includedNames = new Set(modules.map((mod) => mod.name));
  profile.modules = profile.modules.filter((item) => includedNames.has(item.name));
  fs.writeFileSync(path.join(destination, 'build-profile.json5'), JSON.stringify(profile, null, 2) + '\n');
  for (const host of hosts) {
    const target = path.join(destination, path.relative(root, host.file));
    const imports = [];
    const branches = [];
    routes.forEach((route, index) => {
      const local = route.source === host.file;
      const alias = local ? route.buildFunction : `ArkDockRoute_${digest(host.file).slice(0, 8)}_${index}`;
      if (!local) imports.push(`import { ${route.buildFunction} as ${alias} } from ${JSON.stringify(host.imports[index])};`);
      const args = route.arity === 0 ? '' : route.arity === 1 ? 'name' : 'name, param';
      branches.push(`  ${index ? 'else ' : ''}if (name === ${JSON.stringify(route.name)}) {\n    ${alias}(${args})\n  }`);
    });
    // Append the dispatcher outside the struct; source line offsets are recorded explicitly.
    const acceptsParam = routes.some((route) => route.arity === 2);
    const functionText = `\n@Builder\nexport function ${host.dispatcher}(name: string${acceptsParam ? ', param: Object' : ''}) {\n${branches.join('\n')}\n}\n`;
    let transformed = host.text;
    for (const edit of [...host.edits].sort((a, b) => b.offset - a.offset)) transformed = transformed.slice(0, edit.offset) + edit.text + transformed.slice(edit.offset);
    const prefix = imports.join('\n') + '\n';
    transformed = prefix + transformed + functionText;
    parseSource(target, transformed, sdk);
    fs.writeFileSync(target, transformed);
    const stagedMod = path.join(destination, host.mod.relative);
    fs.writeFileSync(path.join(stagedMod, 'oh-package.json5'), JSON.stringify(host.mod.manifest, null, 2) + '\n');
    for (const dependency of added.filter((item) => item.host === host.mod.name)) {
      const depMod = modules.find((mod) => mod.name === dependency.target);
      const link = path.join(stagedMod, 'oh_modules', dependency.alias);
      if (!fs.existsSync(link)) {
        fs.mkdirSync(path.dirname(link), { recursive: true });
        fs.symlinkSync(path.join(destination, depMod.relative), link, process.platform === 'win32' ? 'junction' : 'dir');
      }
    }
    sources.push({ source: host.file, staged: target, sourceHash: digest(host.text), stagedHash: digest(transformed), prefixLines: imports.length, insertions: host.edits.map(({ offset, text }) => ({ offset, length: text.length })) });
  }
  const sourceMapPath = path.join(output, 'source-map.json');
  writeJson(sourceMapPath, { version: 1, originalRoot: root, stagedRoot: destination, files: sources });
  const result = { version: 1, adapted: true, root: destination, module: path.join(destination, selected.relative), routeCount: routes.length, reportPath: report, sourceMapPath, hvigorHome: wrapper.home, wrapperCache: wrapper,
    navigationCount: hosts.reduce((sum, host) => sum + host.edits.length, 0), snapshot: stats, addedDependencies: added,
    routes: routes.map((route) => ({ name: route.name, module: route.module.name, builder: route.buildFunction, sourceHash: route.sourceHash })) };
  writeJson(report, result);
  return result;
}
function main() {
  const args = process.argv.slice(2);
  const options = {};
  const names = { '--project-root': 'projectRoot', '--module': 'moduleName', '--output-root': 'outputRoot', '--sdk-root': 'sdkRoot', '--report': 'reportPath' };
  for (let i = 0; i < args.length; i += 2) {
    if (!names[args[i]] || !args[i + 1] || options[names[args[i]]]) fail('ROUTE_ADAPTER_ARGUMENTS');
    options[names[args[i]]] = args[i + 1];
  }
  if (!options.projectRoot || !options.moduleName || !options.outputRoot || !options.sdkRoot) fail('ROUTE_ADAPTER_ARGUMENTS');
  const result = prepare(options);
  process.stdout.write(JSON.stringify({ adapted: result.adapted, routeCount: result.routeCount }) + '\n');
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { main(); } catch (error) { process.stderr.write(String(error.message).slice(0, 2048) + '\n'); process.exitCode = 1; }
}
