const DEFAULT_URL = 'https://github.com/aarontzhang/peer/releases/latest/download/Peer.dmg';

export default function handler(_req, res) {
  const url = process.env.PEER_MACOS_DOWNLOAD_URL || DEFAULT_URL;
  res.statusCode = 302;
  res.setHeader('location', url);
  res.setHeader('cache-control', 'no-store');
  res.end();
}
