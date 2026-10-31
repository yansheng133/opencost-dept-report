// Jest transform for .vue files, for the component smoke tests only. Uses
// the @vue/compiler-sfc and Babel that come with @rancher/shell, so no
// extra test dependency is installed. Styles are dropped.
const { parse, compileScript, compileTemplate } = require('@vue/compiler-sfc');
const babel = require('@babel/core');

module.exports = {
  process(src, filename) {
    const { descriptor } = parse(src, { filename });
    const id = filename.replace(/[^a-z0-9]/gi, '').slice(-16);
    const script = compileScript(descriptor, { id, inlineTemplate: false });
    const tpl = compileTemplate({
      source: descriptor.template.content, filename, id, compilerOptions: { isTS: true, bindingMetadata: script.bindings }
    });
    const code = `${ script.content.replace('export default', 'const __sfc__ =') }\n${ tpl.code.replace('export function render', 'function render') }\n__sfc__.render = render;\nexport default __sfc__;\n`;

    return babel.transformSync(code, {
      filename:   `${ filename }.ts`,
      configFile: false,
      babelrc:    false,
      presets:    [['@babel/preset-env', { targets: { node: 'current' } }], '@babel/preset-typescript'],
    });
  },
};
