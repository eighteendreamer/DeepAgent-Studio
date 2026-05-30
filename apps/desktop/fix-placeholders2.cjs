const fs = require('fs');
const path = require('path');
const dir = path.join(__dirname, 'src/components/settings');
const files = fs.readdirSync(dir);

files.forEach(file => {
  if (file !== 'GeneralSettings.tsx' && file.endsWith('Settings.tsx')) {
    const p = path.join(dir, file);
    let content = fs.readFileSync(p, 'utf8');
    content = content.replace(/<div>/, '<>');
    // The previous script already replaced the closing tag to </>! So no need to replace closing tag if it is already </>.
    // But let's just make sure.
    fs.writeFileSync(p, content);
  }
});
