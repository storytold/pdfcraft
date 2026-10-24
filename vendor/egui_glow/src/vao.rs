#![expect(unsafe_code)]
#![expect(clippy::unwrap_used)]

use glow::HasContext as _;

use crate::check_for_gl_error;

// ----------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct BufferInfo {
    pub location: u32, //
    pub vector_size: i32,
    pub data_type: u32, //GL_FLOAT,GL_UNSIGNED_BYTE
    pub normalized: bool,
    pub stride: i32,
    pub offset: i32,
}

// ----------------------------------------------------------------------------

/// Wrapper around either Emulated VAO or GL's VAO.
pub(crate) struct VertexArrayObject {
    // If `None`, we emulate VAO:s.
    vao: Option<crate::glow::VertexArray>,
    vbo: glow::Buffer,
    buffer_infos: Vec<BufferInfo>,
}

impl VertexArrayObject {
    pub(crate) unsafe fn new(
        gl: &glow::Context,
        vbo: glow::Buffer,
        buffer_infos: Vec<BufferInfo>,
    ) -> Self {
        let vao = if supports_vao(gl) {
            unsafe {
                let vao = gl.create_vertex_array().unwrap();
                check_for_gl_error!(gl, "create_vertex_array");

                // Store state in the VAO:
                gl.bind_vertex_array(Some(vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));

                for attribute in &buffer_infos {
                    gl.vertex_attrib_pointer_f32(
                        attribute.location,
                        attribute.vector_size,
                        attribute.data_type,
                        attribute.normalized,
                        attribute.stride,
                        attribute.offset,
                    );
                    check_for_gl_error!(gl, "vertex_attrib_pointer_f32");
                    gl.enable_vertex_attrib_array(attribute.location);
                    check_for_gl_error!(gl, "enable_vertex_attrib_array");
                }

                gl.bind_vertex_array(None);

                Some(vao)
            }
        } else {
            log::debug!("VAO not supported");
            None
        };

        Self {
            vao,
            vbo,
            buffer_infos,
        }
    }

    pub(crate) unsafe fn bind(&self, gl: &glow::Context) {
        unsafe {
            if let Some(vao) = self.vao {
                gl.bind_vertex_array(Some(vao));
                check_for_gl_error!(gl, "bind_vertex_array");
            } else {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
                check_for_gl_error!(gl, "bind_buffer");

                for attribute in &self.buffer_infos {
                    gl.vertex_attrib_pointer_f32(
                        attribute.location,
                        attribute.vector_size,
                        attribute.data_type,
                        attribute.normalized,
                        attribute.stride,
                        attribute.offset,
                    );
                    check_for_gl_error!(gl, "vertex_attrib_pointer_f32");
                    gl.enable_vertex_attrib_array(attribute.location);
                    check_for_gl_error!(gl, "enable_vertex_attrib_array");
                }
            }
        }
    }

    pub(crate) unsafe fn unbind(&self, gl: &glow::Context) {
        unsafe {
            if self.vao.is_some() {
                gl.bind_vertex_array(None);
            } else {
                gl.bind_buffer(glow::ARRAY_BUFFER, None);
                for attribute in &self.buffer_infos {
                    gl.disable_vertex_attrib_array(attribute.location);
                }
            }
        }
    }
}

// ----------------------------------------------------------------------------

fn supports_vao(gl: &glow::Context) -> bool {
    const WEBGL_PREFIX: &str = "WebGL ";
    const OPENGL_ES_PREFIX: &str = "OpenGL ES ";

    let version_string = unsafe { gl.get_parameter_string(glow::VERSION) };
    log::debug!("GL version: {version_string:?}.");

    // PdfCraft patch: `PDFCRAFT_GL_LEGACY=1` takes the OpenGL 2.x path on any driver, to test it.
    if std::env::var_os("PDFCRAFT_GL_LEGACY").is_some_and(|v| v == "1") {
        return false;
    }

    // Examples:
    // * "WebGL 2.0 (OpenGL ES 3.0 Chromium)"
    // * "WebGL 2.0"

    if let Some(pos) = version_string.rfind(WEBGL_PREFIX) {
        let version_str = &version_string[pos + WEBGL_PREFIX.len()..];
        if version_str.contains("1.0") {
            // need to test OES_vertex_array_object .
            let supported_extensions = gl.supported_extensions();
            log::debug!("Supported OpenGL extensions: {supported_extensions:?}");
            supported_extensions.contains("OES_vertex_array_object")
                || supported_extensions.contains("GL_OES_vertex_array_object")
        } else {
            true
        }
    } else if version_string.contains(OPENGL_ES_PREFIX) {
        // glow targets es2.0+ so we don't concern about OpenGL ES-CM,OpenGL ES-CL
        if version_string.contains("2.0") {
            // need to test OES_vertex_array_object .
            let supported_extensions = gl.supported_extensions();
            log::debug!("Supported OpenGL extensions: {supported_extensions:?}");
            supported_extensions.contains("OES_vertex_array_object")
                || supported_extensions.contains("GL_OES_vertex_array_object")
        } else {
            true
        }
    } else {
        // from OpenGL 3 vao into core
        if version_string.starts_with('2') {
            // PdfCraft patch: OpenGL 2.x always emulates VAOs, even when the driver advertises
            // ARB_vertex_array_object. The Intel HD Graphics (Ironlake) OpenGL 2.1 driver on
            // Windows 7 (8.15.10.2559) advertises it, and with real VAOs every mesh was drawn as
            // a few huge stretched triangles: no menus, no text, no page. Emulation rebinds the
            // vertex buffer and attribute pointers for each draw, as on WebGL 1, which any 2.x
            // driver handles. See PdfCraft's vendor/README.md.
            log::debug!("OpenGL 2.x: emulating VAOs");
            false
        } else {
            true
        }
    }
}
