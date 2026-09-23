import importlib


def load(name: str):
    return importlib.import_module(name)


def evaluate(source: str):
    return eval(source)


def builtin_import(name: str):
    return __import__(name)
