// Stand-in for @shell/composables/useI18n in the component smoke tests:
// returns the key and its arguments, so a test can tell which text the
// page asked for.
export function useI18n() {
  return { t: (key: string, args?: Record<string, unknown>) => (args && Object.keys(args).length ? `${ key }${ JSON.stringify(args) }` : key) };
}
