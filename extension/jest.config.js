// Unit tests for the extension's pure helpers (pkg/*/utils) and smoke
// tests that mount the components (pkg/*/components/__tests__, in jsdom).
// Jest, Babel, jsdom and @vue/test-utils come with @rancher/shell; no extra
// test dependencies are installed.
const ts = ['babel-jest', {
  configFile: false,
  babelrc:    false,
  presets:    [
    ['@babel/preset-env', { targets: { node: 'current' } }],
    '@babel/preset-typescript',
  ],
}];

module.exports = {
  testEnvironment:          'node',
  roots:                    ['<rootDir>/pkg'],
  testMatch:                ['**/__tests__/**/*.test.ts'],
  modulePathIgnorePatterns: ['<rootDir>/dist-pkg', '/\\.shell/', '/node_modules/'],
  moduleNameMapper:         { '^@shell/composables/useI18n$': '<rootDir>/test/use-i18n-stub.ts' },
  // The browser build of Vue in jsdom, not the server one.
  testEnvironmentOptions:   { customExportConditions: ['node', 'node-addons'] },
  transform:                {
    '^.+\\.ts$':  ts,
    '^.+\\.vue$': '<rootDir>/test/vue-transform.js',
  },
};
