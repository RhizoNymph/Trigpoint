def unique(items):
    literal = {1, 2, 3}
    built = set(items)
    comprehension = {item for item in items}
    return literal, built, comprehension
