import { Plugin } from '@opencode/plugin'
import { installNativeTools } from './enable.mjs'

export default Plugin.define({
  id: 'local.teem.native',
  async setup(context) {
    const dispose = await installNativeTools(context)
    return () => dispose()
  },
})
