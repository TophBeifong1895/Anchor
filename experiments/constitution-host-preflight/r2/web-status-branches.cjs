#!/usr/bin/env node
/** Status-string syntax census for exactly the changed production TS/TSX files.
 * Requires the repository's installed TypeScript (npm --prefix apps/web ci).
 * All revisions use the same file union and vocabulary. Tests/types/comments,
 * CSS and action names are not branches. Boolean-only comparisons are recorded
 * separately so replacing a ternary by && cannot conceal the remaining rule.
 */
const fs = require('node:fs');
const path = require('node:path');
const cp = require('node:child_process');
const root = path.resolve(__dirname, '../../..');
const ts = require(path.join(root, 'apps/web/node_modules/typescript'));
const baseline = '1d1ba54';
const rRevision = '54e664e';
const sourceRevision = process.argv[2];
if (!sourceRevision) throw Error('Usage: node web-status-branches.cjs COMMIT|WORKTREE [output.json]');
const output = process.argv[3] || path.join(__dirname, 'web-branches-draft.json');
const git = args => cp.execFileSync('git', args, { cwd: root, encoding: 'utf8' });
const resolve = revision => revision === 'WORKTREE' ? revision : git(['rev-parse', revision]).trim();
const revisions = { baseline: resolve(baseline), r: resolve(rRevision), r2: resolve(sourceRevision) };
const changedArgs = ['diff', '--name-only', rRevision];
if (sourceRevision !== 'WORKTREE') changedArgs.push(sourceRevision);
changedArgs.push('--', 'apps/web/src');
const changed = git(changedArgs).trim().split('\n').filter(file => /\.(ts|tsx)$/.test(file) && !/\.(test|spec)\.[tj]sx?$/.test(file));
// PilotTurn was named in the task but is a Session Turn consumer. Keep it as an
// explicit, unchanged supplementary scope, never as a claimed Run reduction.
const supplementary = ['apps/web/src/PilotTurn.tsx'].filter(file => !changed.includes(file));
const vocabulary = new Set([
  'ready', 'running', 'paused', 'stopped', 'budget_stopped', 'waiting_recovery',
  'waiting_call', 'completed', 'failed', 'aborted', 'waiting_resume', 'pausing',
  'stopping', 'finished', 'uncertain', 'executing', 'waiting', 'settled', 'human',
  'child', 'effect', 'done', 'escalated', 'unknown', 'unavailable', 'interrupted',
  'pending', 'skipped', 'planned', 'missed_busy', 'missed_downtime', 'cancelled',
  'queued', 'created', 'connecting', 'authenticating', 'authenticated', 'reconnecting',
  'sending', 'waiting_approval', 'waiting_user', 'waiting_event', 'stalled', 'retrying',
  'succeeded', 'registered', 'passed', 'rejected', 'error', 'blocked', 'outcome_unknown',
]);
const read = (revision, file) => revision === 'WORKTREE'
  ? fs.readFileSync(path.join(root, file), 'utf8') : git(['show', `${revision}:${file}`]);

function context(node) {
  for (let item = node; item; item = item.parent) {
    if ((ts.isFunctionDeclaration(item) || ts.isVariableDeclaration(item) || ts.isMethodDeclaration(item)) && item.name) {
      // Local variables are useful source context, followed by their function.
      const name = item.name.getText();
      if (ts.isFunctionDeclaration(item) || ts.isMethodDeclaration(item)
        || (item.initializer && (ts.isArrowFunction(item.initializer) || ts.isFunctionExpression(item.initializer)))) return name;
    }
  }
  return '(module)';
}
function strings(node) {
  const found = new Set();
  const visit = item => {
    if (ts.isStringLiteralLike(item) && vocabulary.has(item.text)) found.add(item.text);
    ts.forEachChild(item, visit);
  };
  visit(node);
  return [...found];
}
function owner(file, node, source, functionName) {
  if (file.endsWith('/PilotTurn.tsx')) return 'Session Turn';
  if (functionName === 'listenerLabel' || /\.listener\b|\blistener\s*===/.test(source)) return 'channel listener';
  if (ts.isPropertyAssignment(node) && ['planned', 'missed_busy', 'missed_downtime'].includes(node.name.text)) return 'schedule observation';
  if (file.endsWith('/execution.ts')) return 'shared execution label table (Run/node/Turn)';
  if (file.endsWith('/WorkflowCanvas.tsx')) return 'Graph node display';
  if (file.endsWith('/runPresentation.ts')) {
    if (functionName === 'runNodePresentation' && (/\bresult\b/.test(source) || /status\s*===\s*'completed'/.test(source))) return 'historical node result';
    return 'Run projection';
  }
  if (file.endsWith('/ChannelsPage.tsx')) {
    if (/delivery\.|pending:\s*'待投递'|sending:\s*'投递中'/.test(source)) return 'channel delivery';
    if (/configurationRead|sessionsRead|pluginUpdate|listener|setConfigurationRead|setSessionsRead|item\.status/.test(source)) return 'channel/UI observation';
    return 'Run projection';
  }
  if (/\bturn\.status\b|\bsession\.status\b/.test(source)) return 'Session Turn';
  if (/\bplan|missed_busy|missed_downtime/.test(source) && !/run\./.test(source)) return 'schedule observation';
  return 'Run projection';
}

function census(revision, file) {
  const source = read(revision, file);
  const ast = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true,
    file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const hits = [], booleanComparisons = [];
  const locate = (node, kind, values, expression = node) => {
    const start = node.getStart(ast), lc = ast.getLineAndCharacterOfPosition(start);
    const expressionSource = expression.getText(ast);
    const functionName = context(node);
    return { file, line: lc.line + 1, column: lc.character + 1, kind,
      values, context: functionName, owner: owner(file, node, expressionSource, functionName),
      source: source.split('\n')[lc.line].trim(), expression: expressionSource };
  };
  const visit = node => {
    if (ts.isIfStatement(node)) {
      const values = strings(node.expression);
      if (values.length) hits.push(locate(node, 'if', values, node.expression));
    } else if (ts.isConditionalExpression(node)) {
      const values = strings(node.condition);
      if (values.length) hits.push(locate(node, 'ternary', values, node.condition));
    } else if (ts.isCaseClause(node)) {
      const values = strings(node.expression);
      if (values.length) hits.push(locate(node, 'switch_case', values, node.expression));
    } else if (ts.isPropertyAssignment(node) && ts.isObjectLiteralExpression(node.parent)) {
      const key = ts.isIdentifier(node.name) || ts.isStringLiteralLike(node.name) ? node.name.text : '';
      if (vocabulary.has(key)) hits.push(locate(node, 'object_key', [key], node.parent));
    }
    if (ts.isBinaryExpression(node) && [ts.SyntaxKind.EqualsEqualsEqualsToken, ts.SyntaxKind.ExclamationEqualsEqualsToken,
      ts.SyntaxKind.EqualsEqualsToken, ts.SyntaxKind.ExclamationEqualsToken].includes(node.operatorToken.kind)) {
      const values = strings(node);
      if (values.length) booleanComparisons.push(locate(node, 'comparison', values));
    } else if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression)
      && node.expression.name.text === 'includes') {
      const values = strings(node);
      if (values.length) booleanComparisons.push(locate(node, 'includes', values));
    }
    ts.forEachChild(node, visit);
  };
  visit(ast);
  const counts = { if: 0, ternary: 0, switch_case: 0, object_key: 0 };
  const byOwner = {};
  for (const hit of hits) {
    counts[hit.kind] += 1;
    byOwner[hit.owner] = (byOwner[hit.owner] || 0) + 1;
  }
  return { points: hits.length, counts, by_owner: byOwner, hits, boolean_comparisons: booleanComparisons };
}

const files = [...new Set([...changed, ...supplementary])].sort();
const result = { revisions, method: 'TypeScript AST: status-string-conditioned if/ternary, matching switch case, matching object-table key; same changed-file union in every revision',
  changed_files: changed.sort(), supplementary_files: supplementary, vocabulary: [...vocabulary].sort(),
  files: {}, total: {}, supplementary_total: {},
  limits: [
    'One if or ternary counts once, even when its condition mentions several status strings.',
    'Object keys count individually; type-union literals, action labels, CSS, JSX attributes and returned status values do not count.',
    'Boolean comparisons/includes (including JSX &&) are a separate inventory, not added to branch totals or treated as deleted rules.',
    'Shared execution labels and unchanged node/Turn/channel/schedule branches remain visible by owner; they are not claimed as Run rules.',
    'A changed production source file added after this snapshot changes the shared file union and requires rerunning the census.',
  ] };
for (const [version, revision] of Object.entries(revisions)) {
  result.files[version] = {};
  result.total[version] = { points: 0, by_owner: {}, by_kind: {}, boolean_comparisons: 0 };
  result.supplementary_total[version] = 0;
  for (const file of files) {
    const row = census(revision, file);
    result.files[version][file] = row;
    if (!changed.includes(file)) { result.supplementary_total[version] += row.points; continue; }
    const total = result.total[version];
    total.points += row.points;
    total.boolean_comparisons += row.boolean_comparisons.length;
    for (const [key, value] of Object.entries(row.by_owner)) total.by_owner[key] = (total.by_owner[key] || 0) + value;
    for (const [key, value] of Object.entries(row.counts)) total.by_kind[key] = (total.by_kind[key] || 0) + value;
  }
}
fs.writeFileSync(output, JSON.stringify(result, null, 2) + '\n');
for (const [version, row] of Object.entries(result.total)) console.log(version, JSON.stringify(row));
console.log('supplementary (excluded from changed-file total)', JSON.stringify(result.supplementary_total));
console.log('output', path.relative(root, output));
