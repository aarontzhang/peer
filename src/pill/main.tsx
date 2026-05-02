import React from 'react';
import { createRoot } from 'react-dom/client';
import '../styles/tokens.css';
import './pill.css';
import { Pill } from './Pill';

const root = createRoot(document.getElementById('root')!);
root.render(
  <React.StrictMode>
    <Pill />
  </React.StrictMode>,
);
