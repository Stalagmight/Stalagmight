// ESLint rule stalagmight/require-disable-reason (#576 RV5-LS-4). Zero tolerance.
//
// An `eslint-disable` directive turns a rule off where the gate would otherwise
// fire, so the reason belongs next to it: after ` -- `, the description ESLint
// itself strips from the directive (docs/linting.md, "Disabling a rule on one
// line"). The convention was documented but nothing enforced it -
// `reportUnusedDisableDirectives` only catches a directive that suppresses
// nothing or names an unknown rule.
//
// Flags every `eslint-disable`, `eslint-disable-line` and
// `eslint-disable-next-line` comment (line or block) with no text after its
// ` -- ` separator:
//   // eslint-disable-next-line @typescript-eslint/no-explicit-any
//   /* eslint-disable-next-line @typescript-eslint/no-explicit-any -- */
// Passes:
//   // eslint-disable-next-line @typescript-eslint/no-explicit-any -- the wire value is untyped JSON
// `eslint-enable` needs no reason, and a comment that merely mentions the word is
// not a directive (the directive must open the comment). [#576 RV6a-7] Nor is a
// LINE comment that opens with plain `eslint-disable`: ESLint takes a `//` comment
// as a directive only when it is `eslint-disable-line` / `eslint-disable-next-line`,
// so `// eslint-disable is not allowed here` is prose to it, and to this rule.
//
// One shape it cannot see: a bare `/* eslint-disable */` turns off every rule for
// the rest of the file, this one included. Review catches that one.
//
// Pinned by scripts/check-eslint.test.mjs.

const DIRECTIVE = /^\s*eslint-disable(?:-next-line|-line)?(?=\s|$)/;
// What ESLint accepts as a directive in a `//` comment.
const LINE_DIRECTIVE = /^\s*eslint-disable-(?:next-)?line(?=\s|$)/;
// ESLint's own separator: two or more dashes with whitespace around them, then
// the description.
const REASON = /\s-{2,}\s+\S/;

export default {
  meta: {
    type: 'suggestion',
    docs: { description: 'Require a reason after ` -- ` on every eslint-disable directive' },
    schema: [],
    messages: {
      missingReason:
        'This eslint-disable directive needs a reason after ` -- ` (e.g. ' +
        '`// eslint-disable-next-line <rule> -- why the rule is wrong here`).',
    },
  },
  create(context) {
    return {
      Program() {
        for (const comment of context.sourceCode.getAllComments()) {
          if (!(comment.type === 'Line' ? LINE_DIRECTIVE : DIRECTIVE).test(comment.value)) continue;
          if (REASON.test(comment.value)) continue;
          context.report({ loc: comment.loc, messageId: 'missingReason' });
        }
      },
    };
  },
};
