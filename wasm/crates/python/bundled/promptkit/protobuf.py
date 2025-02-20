import _promptkit_protobuf

pool = _promptkit_protobuf.new()


def load_file_descriptor_set(file):
    pool.add_descriptor(file)


def encode_message(name, message):
    return pool.encode(name, message)


def decode_message(name, data):
    return pool.decode(name, data)
