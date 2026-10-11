import { createApp } from 'vue'
import App from './App.vue'

// Carbon Vue uses Carbon v10 classnames (bx--), so import matching v10 stylesheet.
import 'carbon-components/css/carbon-components.css'
import './style.css'

// @carbon/vue's package entry is a single prebuilt bundle of every component (~530 kB), which
// cannot be tree-shaken. Import only the components App.vue uses, from their Vue sources.
import CvButton from '@carbon/vue/src/components/CvButton/CvButton.vue'
import CvCheckbox from '@carbon/vue/src/components/CvCheckbox/CvCheckbox.vue'
import CvSearch from '@carbon/vue/src/components/CvSearch/CvSearch.vue'
import CvSelect from '@carbon/vue/src/components/CvSelect/CvSelect.vue'
import CvSelectOption from '@carbon/vue/src/components/CvSelect/CvSelectOption.vue'
import CvSlider from '@carbon/vue/src/components/CvSlider/CvSlider.vue'
import CvTag from '@carbon/vue/src/components/CvTag/CvTag.vue'

const app = createApp(App)

app.component('CvButton', CvButton)
app.component('CvCheckbox', CvCheckbox)
app.component('CvSearch', CvSearch)
app.component('CvSelect', CvSelect)
app.component('CvSelectOption', CvSelectOption)
app.component('CvSlider', CvSlider)
app.component('CvTag', CvTag)

app.mount('#app')
