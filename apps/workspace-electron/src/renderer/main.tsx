import React from 'react';
import { createRoot } from 'react-dom/client';

import './globals.css';
import { Root } from '@/app/root';

const root = document.getElementById('root');
if (!root) {
  throw new Error('missing root element');
}

createRoot(root).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
