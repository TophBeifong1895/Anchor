import os, zipfile, pathlib, hashlib, base64
def get_requires_for_build_editable(config_settings=None): return []
def build_editable(wheel_directory, config_settings=None, metadata_directory=None):
    assert pathlib.Path.cwd() == pathlib.Path('/workspace/one')
    assert not pathlib.Path(os.environ.get('ANCHOR_RUNNER_STATE_ROOT', '/no-host-state')).exists()
    files = {
      'editableprobe.pth': b'/workspace/one/src\n',
      'editableprobe-1.0.dist-info/METADATA': b"Metadata-Version: 2.1\nName: editableprobe\nVersion: 1.0\nRequires-Dist: probepkg==1.0; python_version >= '3.8'\nProvides-Extra: dev\nRequires-Dist: trialpkg==1.0; extra == 'dev'\n",
      'editableprobe-1.0.dist-info/WHEEL': b'Wheel-Version: 1.0\nGenerator: anchor-fixture\nRoot-Is-Purelib: true\nTag: py3-none-any\n',
      'editableprobe-1.0.dist-info/entry_points.txt': b'[console_scripts]\neditable-probe = editableprobe:main\n',
    }
    record=''.join(f'{p},sha256={base64.urlsafe_b64encode(hashlib.sha256(v).digest()).decode().rstrip("=")},{len(v)}\n' for p,v in files.items())
    files['editableprobe-1.0.dist-info/RECORD'] = (record+'editableprobe-1.0.dist-info/RECORD,,\n').encode()
    filename='editableprobe-1.0-py3-none-any.whl'
    with zipfile.ZipFile(pathlib.Path(wheel_directory)/filename,'w') as wheel:
        for path, content in files.items(): wheel.writestr(path,content)
    return filename
