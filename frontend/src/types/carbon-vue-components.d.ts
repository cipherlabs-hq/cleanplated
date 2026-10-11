// Per-component imports from @carbon/vue's Vue sources (see main.ts) ship without types.
declare module '@carbon/vue/src/components/*.vue' {
  import type { DefineComponent } from 'vue'
  const component: DefineComponent
  export default component
}
