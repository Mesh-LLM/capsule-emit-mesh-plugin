// The capsules plugin's mesh-llm plugin web UI bundle.
//
// The console imports this module from the installed package (manifest
// `web_ui` block: bundle `main` rooted at `bundle/`, page `evidence`) and calls
// `registerMeshPluginUi(host)`; see docs/plugins/README.md "Plugin Web UI
// Projection Contract" in Mesh-LLM/mesh-llm. Each mount returns
// `{ unmount() }`, which tears down the React root, the injected stylesheet,
// and the mounted host.
import { createRoot } from 'react-dom/client'
import evidenceCss from '@/styles/evidence.css?inline'
import { EvidencePage, focusExchangeKeyFrom } from '@/features/capsules/pages/EvidencePage'
import { setPluginHost } from '@/plugin-host/host'
import type {
  MeshPluginUiBundleModule,
  MeshPluginUiMountContext,
  MeshPluginUiMountHandle
} from '@/plugin-host/host-contract'

export const EVIDENCE_PAGE_ID = 'evidence'
const STYLE_ELEMENT_ID = 'capsules-evidence-styles'

function injectStyles(): HTMLStyleElement {
  const existing = document.getElementById(STYLE_ELEMENT_ID)
  existing?.remove()
  const style = document.createElement('style')
  style.id = STYLE_ELEMENT_ID
  style.textContent = evidenceCss
  document.head.appendChild(style)
  return style
}

export function mountEvidencePage({ element, host }: MeshPluginUiMountContext): MeshPluginUiMountHandle {
  setPluginHost(host)
  const style = injectStyles()
  const container = document.createElement('div')
  element.replaceChildren(container)
  const root = createRoot(container)
  root.render(<EvidencePage focusExchangeKey={focusExchangeKeyFrom(window.location.search)} />)
  return {
    unmount() {
      root.unmount()
      container.remove()
      style.remove()
      setPluginHost(null)
    }
  }
}

const moduleRegistration: MeshPluginUiBundleModule = {
  registerMeshPluginUi() {
    return { pages: { [EVIDENCE_PAGE_ID]: mountEvidencePage } }
  }
}

export const registerMeshPluginUi = moduleRegistration.registerMeshPluginUi
