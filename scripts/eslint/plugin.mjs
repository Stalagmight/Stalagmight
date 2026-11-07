// The repo's own ESLint rules, registered as the `stalagmight` plugin in
// eslint.config.js. Each rule file documents what it flags and why; the
// fixtures are in scripts/check-eslint.test.mjs.
import noRenderRefWrite from './no-render-ref-write.mjs';
import requireStoreSelector from './require-store-selector.mjs';
import requireDisableReason from './require-disable-reason.mjs';

export default {
  meta: { name: 'stalagmight' },
  rules: {
    'no-render-ref-write': noRenderRefWrite,
    'require-store-selector': requireStoreSelector,
    'require-disable-reason': requireDisableReason,
  },
};
