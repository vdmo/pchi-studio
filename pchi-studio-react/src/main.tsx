import React from 'react'
import ReactDOM from 'react-dom/client'
import '@xyflow/react/dist/style.css'
import './styles.css'
import App from './App'
import DeptPage from './components/DeptPage'

// /?dept=lighting (or a device id) opens the small page for one department's desk or tablet
const params = new URLSearchParams(location.search)
const who = params.get('dept')?.trim().toLowerCase()
const token = params.get('token')
if (token) {
  try {
    localStorage.setItem('pchi.token', token)
    params.delete('token')
    history.replaceState(null, '', `${location.pathname}${params.toString() ? `?${params}` : ''}`) // keep the token out of the address bar
  } catch {
    /* storage blocked: the page will report that it cannot connect */
  }
}

ReactDOM.createRoot(document.getElementById('root')!).render(<React.StrictMode>{who ? <DeptPage who={who} /> : <App />}</React.StrictMode>)
