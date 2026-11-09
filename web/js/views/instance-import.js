/**
 * "Import a server": folders an admin placed in the app's imports folder, each with what it
 * looks like (loader, versions, jar). The user confirms or corrects that, accepts the EULA, and
 * the folder is copied into a new, sandboxed server. A modpack or a zipped server can be uploaded
 * here too.
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { onSubmit } from '../core/forms.js';
import { navigate } from '../core/navigation.js';

export async function mount(root) {
  const list = h('div', { class: 'c-import__list' });
  root.append(
    h('header', { class: 'l-page-header' },
      h('h1', { class: 'l-page-header__title' }, 'Import a server'),
      h('a', { class: 'c-button c-button--ghost', href: '/servers/new' }, 'Back'),
    ),
    list,
  );

  let view;
  let versions = [];
  try {
    [view, versions] = await Promise.all([api.get('/imports'), api.get('/catalog/minecraft/versions')]);
  } catch (error) {
    list.replaceChildren(h('p', { class: 'c-muted' }, error.message));
    return;
  }
  const releases = versions.filter((v) => v.kind === 'release').map((v) => v.id);
  // Loader ids and display names come from the server (they live in one place there).
  const loaders = releases.length > 0
    ? await api.get(`/catalog/minecraft/loaders?mc=${encodeURIComponent(releases[0])}`).catch(() => [])
    : [];

  const upload = createUpload(() => {
    // A server zip is now waiting among the folders below.
    root.replaceChildren();
    mount(root);
  });
  const help = h('section', { class: 'c-card c-import__help' },
    h('p', null, 'Copy a server folder into ', h('code', null, view.folder), ' on the machine (or container) running this app, then reload this page. With Docker: ',
      h('code', null, 'docker cp ./my-server <container>:' + view.folder + '/my-server'), '.'),
    h('p', { class: 'c-muted' }, 'The folder is copied into a new server; the original is left as it is.'),
  );
  if (view.candidates.length === 0) {
    list.replaceChildren(upload, help, h('p', { class: 'c-muted' }, 'No folders are waiting to be imported.'));
    return;
  }
  list.replaceChildren(upload, help, ...view.candidates.map((candidate) => card(candidate, releases, loaders)));
}

/**
 * Uploading a modpack (`.mrpack`, a CurseForge zip) or a zipped server folder. A pack shows what it
 * needs and a form to create its server; a server zip joins the folders on this page
 * (`onServerFolder`).
 */
function createUpload(onServerFolder) {
  const status = h('div', { class: 'c-import__upload-result' });
  const input = h('input', { type: 'file', name: 'modpack_file', accept: '.mrpack,.zip', onChange: async () => {
    const [file] = input.files;
    if (!file) return;
    input.disabled = true;
    status.replaceChildren(h('p', { class: 'c-muted' }, `Uploading ${file.name}…`));
    try {
      const uploaded = await api.upload(`/modpacks/upload?file=${encodeURIComponent(file.name)}`, file);
      if (uploaded.kind === 'server_folder') onServerFolder();
      else status.replaceChildren(packForm(uploaded));
    } catch (error) {
      status.replaceChildren(h('p', { class: 'c-form__error', role: 'alert' }, error.message));
    } finally {
      input.disabled = false;
      input.value = '';
    }
  } });
  return h('section', { class: 'c-card c-import__card' },
    h('h2', { class: 'c-import__title' }, 'Upload a modpack or a server'),
    h('p', { class: 'c-muted' }, 'A Modrinth pack (.mrpack), a CurseForge pack, or a zip of a server folder.'),
    h('label', { class: 'c-field' }, h('span', { class: 'c-field__label' }, 'File'), input),
    status);
}

/** What an uploaded pack needs, and the form that creates a server for it. */
function packForm({ upload_id: uploadId, pack }) {
  const version = pack.loader_version ? ` ${pack.loader_version}` : '';
  const form = h('form', { class: 'c-form', novalidate: true, dataset: { pack: uploadId } },
    h('h3', { class: 'c-import__title' }, pack.version ? `${pack.name} ${pack.version}` : pack.name),
    h('ul', { class: 'c-import__notes' },
      h('li', null, `Minecraft ${pack.mc_version}, ${pack.loader}${version}`),
      h('li', null, `${pack.files} files to download`)),
    h('div', { class: 'c-field' }, h('label', { class: 'c-field__label', for: 'pack-name' }, 'Server name'),
      h('input', { class: 'c-input', id: 'pack-name', name: 'name', maxlength: 64, value: pack.name })),
    h('div', { class: 'c-field' }, h('label', { class: 'c-field__label', for: 'pack-memory' }, 'Maximum memory (MB)'),
      h('input', { class: 'c-input', id: 'pack-memory', name: 'max_memory_mb', type: 'number', min: 512, max: 65536, step: 256, value: 4096 })),
    h('label', { class: 'c-check' }, h('input', { type: 'checkbox', name: 'eula_accepted', value: 'yes' }),
      h('span', null, 'I accept the ', h('a', { href: 'https://aka.ms/MinecraftEULA', target: '_blank', rel: 'noopener noreferrer' }, 'Minecraft EULA'), '.')),
    h('p', { class: 'c-form__error', 'data-form-error': '', role: 'alert', hidden: true }),
    h('div', { class: 'c-form__actions' }, h('button', { class: 'c-button c-button--primary', type: 'submit' }, 'Create the server')),
  );
  onSubmit(form, async (values) => {
    const { instance } = await api.post('/modpacks/install', {
      upload_id: uploadId,
      name: values.name,
      max_memory_mb: Number(values.max_memory_mb),
      eula_accepted: values.eula_accepted === 'yes',
    });
    navigate(`/servers/${instance.id}`);
  });
  return form;
}

function card(candidate, releases, loaders) {
  const detected = candidate.detected;
  const id = (name) => `import-${candidate.folder}-${name}`.replace(/[^a-z0-9-]/gi, '-');
  const versionOptions = [...new Set([detected.mc_version, ...releases].filter(Boolean))];
  const select = (name, options, value, labels = {}) => {
    const element = h('select', { class: 'c-input', id: id(name), name },
      options.map((option) => h('option', { value: option }, labels[option] ?? option)));
    if (value) element.value = value;
    return element;
  };
  const field = (label, control, hint = null) => h('div', { class: 'c-field' },
    h('label', { class: 'c-field__label', for: control.id }, label), control,
    hint ? h('p', { class: 'c-field__hint' }, hint) : null);

  const form = h('form', { class: 'c-form', novalidate: true, dataset: { folder: candidate.folder } },
    h('h2', { class: 'c-import__title' }, candidate.folder),
    h('ul', { class: 'c-import__notes' }, detected.notes.map((note) => h('li', null, note))),
    field('Server name', h('input', { class: 'c-input', id: id('name'), name: 'name', maxlength: 64, value: candidate.folder })),
    field('Minecraft version', select('mc_version', versionOptions, detected.mc_version)),
    field('Server software', select(
      'loader',
      loaders.map((entry) => entry.loader),
      detected.loader,
      Object.fromEntries(loaders.map((entry) => [entry.loader, entry.name])),
    )),
    field('Jar that starts it', h('input', { class: 'c-input c-input--mono', id: id('jar'), name: 'jar', value: detected.jar ?? '' }),
      'Not needed for Forge and NeoForge (they start from their run.sh).'),
    field('Maximum memory (MB)', h('input', { class: 'c-input', id: id('memory'), name: 'max_memory_mb', type: 'number', min: 512, max: 65536, step: 256, value: 2048 })),
    h('label', { class: 'c-check' }, h('input', { type: 'checkbox', name: 'eula_accepted', value: 'yes' }),
      h('span', null, 'I accept the ', h('a', { href: 'https://aka.ms/MinecraftEULA', target: '_blank', rel: 'noopener noreferrer' }, 'Minecraft EULA'), '.')),
    h('p', { class: 'c-form__error', 'data-form-error': '', role: 'alert', hidden: true }),
    h('div', { class: 'c-form__actions' }, h('button', { class: 'c-button c-button--primary', type: 'submit' }, 'Import')),
  );
  onSubmit(form, async (values) => {
    const { instance } = await api.post('/imports', {
      folder: candidate.folder,
      name: values.name,
      mc_version: values.mc_version,
      loader: values.loader,
      loader_version: detected.loader_version,
      jar: values.jar,
      max_memory_mb: Number(values.max_memory_mb),
      eula_accepted: values.eula_accepted === 'yes',
    });
    navigate(`/servers/${instance.id}`);
  });
  return h('section', { class: 'c-card c-import__card' }, form);
}
