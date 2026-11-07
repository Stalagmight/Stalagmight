// ESLint rule stalagmight/no-render-ref-write (#576 D17).
//
// Flags `x.current = ...` executed while a component or hook renders: in its
// body, in a nested block of it, or in a useMemo / useState / useReducer
// initialiser callback (those run during render too). A render React throws
// away (an error boundary retry, a suspended or interrupted concurrent render)
// still leaves such a write behind, so a handler or effect can act on a value
// the screen never showed. The fix is useLatest(value) (src/hooks/useLatest.ts)
// for a "latest value" mirror, or a write in an effect or event handler.
//
// What counts as a component or hook: a function whose own name, or the name
// of the variable it is assigned to (through memo()/forwardRef() wrappers),
// starts with `use` + an upper-case letter or digit, or with an upper-case
// letter - the same naming rule react-hooks/rules-of-hooks relies on.
//
// Not flagged:
//   - writes in nested functions (handlers, effect callbacks, timers): they run
//     after the render committed, if at all;
//   - the lazy initialisation React documents, `if (ref.current === null)
//     ref.current = make();` (also `== null`, `=== undefined`, `!ref.current`),
//     and `ref.current ??= make()` / `ref.current ||= make()`: idempotent, so a
//     discarded render cannot leave a wrong value behind.
//
// Pinned by scripts/check-eslint.test.mjs.

const WRAPPERS = new Set(['memo', 'forwardRef']);
const RENDER_TIME_CALLBACKS = new Set(['useMemo', 'useState', 'useReducer']);
const COMPONENT_OR_HOOK = /^(use[A-Z0-9]|[A-Z])/;
const FUNCTION_TYPES = new Set(['FunctionDeclaration', 'FunctionExpression', 'ArrowFunctionExpression']);

const calleeName = (call) => {
  const c = call.callee;
  if (c.type === 'Identifier') return c.name;
  if (c.type === 'MemberExpression' && !c.computed && c.property.type === 'Identifier') return c.property.name;
  return null;
};

// The name a function renders under, or null for an anonymous callback.
function renderFunctionName(fn) {
  if (fn.id?.name) return fn.id.name;
  let p = fn.parent;
  while (p?.type === 'CallExpression' && WRAPPERS.has(calleeName(p))) p = p.parent;
  if (p?.type === 'VariableDeclarator' && p.id.type === 'Identifier') return p.id.name;
  return null;
}

// Is `fn` a callback React runs during render (useMemo's factory, a lazy
// useState / useReducer initialiser)?
function runsDuringRender(fn) {
  const p = fn.parent;
  return p?.type === 'CallExpression' && RENDER_TIME_CALLBACKS.has(calleeName(p)) && p.arguments.includes(fn);
}

function isNullCheckOf(test, target, sourceCode) {
  const same = (n) => n?.type === 'MemberExpression' && sourceCode.getText(n) === target;
  const nullish = (n) => (n?.type === 'Literal' && n.value === null) || (n?.type === 'Identifier' && n.name === 'undefined');
  if (test.type === 'UnaryExpression' && test.operator === '!') return same(test.argument);
  if (test.type === 'BinaryExpression' && (test.operator === '===' || test.operator === '==')) {
    return (same(test.left) && nullish(test.right)) || (same(test.right) && nullish(test.left));
  }
  return false;
}

export default {
  meta: {
    type: 'problem',
    docs: { description: 'Disallow writing `ref.current` while a component or hook renders' },
    schema: [],
    messages: {
      renderWrite:
        'Do not write `{{target}}` while `{{fn}}` renders: a render React discards would leave the value behind. ' +
        'Use useLatest(value) (src/hooks/useLatest.ts), or write it in an effect or event handler.',
    },
  },
  create(context) {
    const sourceCode = context.sourceCode;
    return {
      AssignmentExpression(node) {
        const left = node.left;
        if (left.type !== 'MemberExpression' || left.computed) return;
        if (left.property.type !== 'Identifier' || left.property.name !== 'current') return;
        if (node.operator === '??=' || node.operator === '||=') return;
        const target = sourceCode.getText(left);
        let child = node;
        for (let p = node.parent; p; child = p, p = p.parent) {
          if (p.type === 'IfStatement' && child === p.consequent && isNullCheckOf(p.test, target, sourceCode)) return;
          if (!FUNCTION_TYPES.has(p.type)) continue;
          if (runsDuringRender(p)) continue; // keep climbing to the component/hook
          const name = renderFunctionName(p);
          if (name && COMPONENT_OR_HOOK.test(name)) {
            context.report({ node, messageId: 'renderWrite', data: { target, fn: name } });
          }
          return;
        }
      },
    };
  },
};
