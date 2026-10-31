/**
 * @jest-environment jsdom
 */
import {
  describe, it, expect, jest
} from '@jest/globals';
import { mount } from '@vue/test-utils';
import Eff from '../Eff.vue';

jest.mock('vuex', () => ({ useStore: () => ({}) }));

describe('efficiency bar', () => {
  it('over and under the request are marked differently; a small value still shows', () => {
    const over = mount(Eff, { props: { value: 3.2 } });
    const under = mount(Eff, { props: { value: 0.004 } });
    const zero = mount(Eff, { props: { value: 0 } });
    const none = mount(Eff, { props: { value: null } });

    expect(over.classes()).toContain('over');
    expect(over.classes()).not.toContain('under');
    expect(under.classes()).toContain('under');
    expect(under.find('.cr-eff-fill').classes()).toContain('some');
    expect(zero.find('.cr-eff-fill').classes()).not.toContain('some');
    expect(none.text()).toContain('costReport.workloads.noRequest');
  });
});
