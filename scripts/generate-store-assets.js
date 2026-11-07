const fs = require('fs');
const path = require('path');

const logoBase64 = fs.readFileSync(path.join(__dirname, '../store-submission/assets/marknexia-512.png')).toString('base64');
const logoDataUrl = `data:image/png;base64,${logoBase64}`;

console.log('Logo loaded, base64 length:', logoBase64.length);
