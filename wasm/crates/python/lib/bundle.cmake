# modified from
# https://gist.github.com/cristianadam/ef920342939a89fae3e8a85ca9459b49
function(bundle_static_library tgt_name bundled_tgt_name)
  list(APPEND static_libs ${tgt_name})

  function(_recursively_collect_dependencies input_target)
    get_target_property(public_dependencies ${input_target}
                        INTERFACE_LINK_LIBRARIES)
    if(NOT public_dependencies)
      return()
    endif()
    foreach(dependency IN LISTS public_dependencies)
      if(TARGET ${dependency})
        get_target_property(alias ${dependency} ALIASED_TARGET)
        if(TARGET ${alias})
          set(dependency ${alias})
        endif()
        get_target_property(_type ${dependency} TYPE)
        if(${_type} STREQUAL "STATIC_LIBRARY")
          list(PREPEND static_libs ${dependency})
        endif()

        get_property(library_already_added GLOBAL
                     PROPERTY _${tgt_name}_static_bundle_${dependency})
        if(NOT library_already_added)
          set_property(GLOBAL PROPERTY _${tgt_name}_static_bundle_${dependency}
                                       ON)
          _recursively_collect_dependencies(${dependency})
        endif()
      else()
        list(PREPEND static_libs ${dependency})
      endif()
    endforeach()
    set(static_libs
        ${static_libs}
        PARENT_SCOPE)
  endfunction()

  _recursively_collect_dependencies(${tgt_name})

  list(REMOVE_DUPLICATES static_libs)

  set(bundled_tgt_full_name
      ${CMAKE_BINARY_DIR}/${CMAKE_STATIC_LIBRARY_PREFIX}${bundled_tgt_name}${CMAKE_STATIC_LIBRARY_SUFFIX}
  )

  if(CMAKE_CXX_COMPILER_ID MATCHES "^(Clang|GNU)$")
    file(WRITE ${CMAKE_BINARY_DIR}/${bundled_tgt_name}.ar.in
         "CREATE ${bundled_tgt_full_name}\n")

    foreach(tgt IN LISTS static_libs)
      if(TARGET ${tgt})
        file(APPEND ${CMAKE_BINARY_DIR}/${bundled_tgt_name}.ar.in
             "ADDLIB $<TARGET_FILE:${tgt}>\n")
      else()
        file(APPEND ${CMAKE_BINARY_DIR}/${bundled_tgt_name}.ar.in
             "ADDLIB ${tgt}\n")
      endif()
    endforeach()

    file(APPEND ${CMAKE_BINARY_DIR}/${bundled_tgt_name}.ar.in "SAVE\n")
    file(APPEND ${CMAKE_BINARY_DIR}/${bundled_tgt_name}.ar.in "END\n")

    file(
      GENERATE
      OUTPUT ${CMAKE_BINARY_DIR}/${bundled_tgt_name}.ar
      INPUT ${CMAKE_BINARY_DIR}/${bundled_tgt_name}.ar.in)

    set(ar_tool ${CMAKE_AR})
    if(CMAKE_INTERPROCEDURAL_OPTIMIZATION)
      set(ar_tool ${CMAKE_CXX_COMPILER_AR})
    endif()

    add_custom_command(
      COMMAND ${ar_tool} -M < ${CMAKE_BINARY_DIR}/${bundled_tgt_name}.ar
      OUTPUT ${bundled_tgt_full_name}
      COMMENT "Bundling ${bundled_tgt_name}"
      VERBATIM)
  else()
    message(FATAL_ERROR "Unknown bundle scenario!")
  endif()

  add_custom_target(${bundled_tgt_name}-bundle ALL
                    DEPENDS ${bundled_tgt_full_name})
  add_dependencies(${bundled_tgt_name}-bundle ${tgt_name})

  add_library(${bundled_tgt_name} STATIC IMPORTED)
  set_target_properties(
    ${bundled_tgt_name}
    PROPERTIES IMPORTED_LOCATION ${bundled_tgt_full_name}
               INTERFACE_INCLUDE_DIRECTORIES
               $<TARGET_PROPERTY:${tgt_name},INTERFACE_INCLUDE_DIRECTORIES>)
  add_dependencies(${bundled_tgt_name} ${bundled_tgt_name}-bundle)
  install(FILES ${bundled_tgt_full_name} DESTINATION lib)
  install(FILES $<TARGET_PROPERTY:${tgt_name},INTERFACE_INCLUDE_DIRECTORIES>
          DESTINATION include)

endfunction()
