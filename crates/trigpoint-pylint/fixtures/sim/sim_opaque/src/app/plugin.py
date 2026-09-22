import json
import thirdparty.widget


def load(path):
    return json.loads(thirdparty.widget.read(path))
